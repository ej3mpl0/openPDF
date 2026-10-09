use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use image::{ImageBuffer, Rgba};
use openpdf_core::{
    merge_paths, parse_page_ranges, split, split_all, Document, Engine, PageRange,
};

#[derive(Parser)]
#[command(
    name = "openpdf",
    about = "openPDF command-line tools (share openpdf-core with the GTK app)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Page count, sizes, and outline
    Info { file: PathBuf },
    /// Render a page to PNG
    Render {
        file: PathBuf,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value_t = 1.0)]
        zoom: f32,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Extract text
    Text {
        file: PathBuf,
        #[arg(long)]
        page: Option<u32>,
    },
    /// Merge two or more PDFs
    Merge {
        files: Vec<PathBuf>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Split a PDF into one file per page (or per --pages range)
    Split {
        file: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// 1-based ranges, e.g. 1-3,5
        #[arg(long)]
        pages: Option<String>,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("openpdf: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Info { file } => info(&file),
        Command::Render {
            file,
            page,
            zoom,
            output,
        } => render(&file, page, zoom, &output),
        Command::Text { file, page } => text(&file, page),
        Command::Merge { files, output } => merge(&files, &output),
        Command::Split {
            file,
            output,
            pages,
        } => split_cmd(&file, &output, pages.as_deref()),
    }
}

fn info(path: &Path) -> Result<()> {
    let doc = Document::open(path).with_context(|| format!("open {}", path.display()))?;
    println!("file: {}", path.display());
    println!("pages: {}", doc.page_count());
    for page in doc.pages() {
        println!(
            "page {}: {:.1} x {:.1} pt  rot {}",
            page.index + 1,
            page.width_pt,
            page.height_pt,
            page.rotation
        );
    }
    let outline = doc.outline().context("outline")?;
    if outline.is_empty() {
        println!("outline: (none)");
    } else {
        println!("outline:");
        print_outline(&outline, 0);
    }
    Ok(())
}

fn print_outline(items: &[openpdf_core::OutlineItem], depth: usize) {
    for item in items {
        let pad = "  ".repeat(depth + 1);
        match item.page {
            Some(p) => println!("{pad}{} -> {}", item.title, p + 1),
            None => println!("{pad}{}", item.title),
        }
        print_outline(&item.children, depth + 1);
    }
}

fn engine() -> Result<Engine> {
    Engine::new().context("load libpdfium (set OPENPDF_PDFIUM or install it on the system)")
}

fn render(path: &Path, page_1: u32, zoom: f32, output: &Path) -> Result<()> {
    if page_1 == 0 {
        bail!("--page is 1-based");
    }
    let engine = engine()?;
    let doc = engine.open(path)?;
    let raster = engine.render_page(doc.id(), page_1 - 1, zoom)?;
    let img = ImageBuffer::<Rgba<u8>, _>::from_raw(raster.width, raster.height, raster.rgba)
        .context("RGBA buffer size")?;
    img.save(output)
        .with_context(|| format!("write {}", output.display()))?;
    println!("wrote {}", output.display());
    Ok(())
}

fn text(path: &Path, page_1: Option<u32>) -> Result<()> {
    let engine = engine()?;
    let doc = engine.open(path)?;
    match page_1 {
        None => {
            for i in 0..doc.page_count() {
                if i > 0 {
                    println!("\n-----");
                }
                print!("{}", engine.page_text(doc.id(), i)?);
            }
        }
        Some(0) => bail!("--page is 1-based"),
        Some(p) => print!("{}", engine.page_text(doc.id(), p - 1)?),
    }
    Ok(())
}

fn merge(files: &[PathBuf], output: &Path) -> Result<()> {
    if files.len() < 2 {
        bail!("merge needs at least two input files and -o");
    }
    let mut doc = merge_paths(files)?;
    doc.save(output)?;
    println!("wrote {} ({} pages)", output.display(), doc.page_count());
    Ok(())
}

fn split_cmd(file: &Path, output: &Path, pages: Option<&str>) -> Result<()> {
    let doc = Document::open(file)?;
    let ranges: Option<Vec<PageRange>> = pages
        .map(|spec| parse_page_ranges(spec, doc.page_count()))
        .transpose()?;
    let parts = match ranges.as_deref() {
        Some(r) => split(&doc, r)?,
        None => split_all(&doc)?,
    };

    if parts.len() == 1 && looks_like_pdf(output) {
        let mut part = parts.into_iter().next().unwrap();
        part.save(output)?;
        println!("wrote {}", output.display());
        return Ok(());
    }

    let dir = output;
    fs::create_dir_all(dir)?;
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    for (i, mut part) in parts.into_iter().enumerate() {
        let dest = dir.join(format!("{}-{}.pdf", stem, i + 1));
        part.save(&dest)?;
        println!("wrote {}", dest.display());
    }
    Ok(())
}

fn looks_like_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}
