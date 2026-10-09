use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{value_parser, Arg, ArgAction, Command};
use image::{ImageBuffer, Rgba};
use openpdf_core::{
    init_i18n, merge_paths, parse_page_ranges, split, split_all, tr, tr_args, Document, Engine,
    PageRange,
};

fn intern(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn l(msgid: &str) -> &'static str {
    intern(tr(msgid))
}

fn help_template() -> String {
    format!(
        "{{about-with-newline}}\n{} {{usage}}\n\n{{all-args}}{{after-help}}",
        tr("Usage:")
    )
}

fn decorate(cmd: Command) -> Command {
    cmd.help_template(help_template())
        .subcommand_help_heading(l("Commands:"))
        .next_help_heading(l("Options:"))
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(l("Print help")),
        )
}

fn cli() -> Command {
    let arguments = l("Arguments:");
    let cmd = decorate(
        Command::new("openpdf")
            .about(tr(
                "openPDF command-line tools (share openpdf-core with the GTK app)",
            ))
            .version(env!("CARGO_PKG_VERSION"))
            .disable_version_flag(true)
            .arg(
                Arg::new("version")
                    .short('V')
                    .long("version")
                    .action(ArgAction::Version)
                    .help(l("Print version")),
            )
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(decorate(
                Command::new("info")
                    .about(tr("Page count, sizes, and outline"))
                    .arg(
                        Arg::new("file")
                            .required(true)
                            .value_name("FILE")
                            .help_heading(arguments)
                            .help(tr("PDF file")),
                    ),
            ))
            .subcommand(decorate(
                Command::new("render")
                    .about(tr("Render a page to PNG"))
                    .arg(
                        Arg::new("file")
                            .required(true)
                            .value_name("FILE")
                            .help_heading(arguments)
                            .help(tr("PDF file")),
                    )
                    .arg(
                        Arg::new("page")
                            .long("page")
                            .value_parser(value_parser!(u32))
                            .default_value("1")
                            .help(tr("Page number (1-based)")),
                    )
                    .arg(
                        Arg::new("zoom")
                            .long("zoom")
                            .value_parser(value_parser!(f32))
                            .default_value("1.0")
                            .help(tr("Zoom factor")),
                    )
                    .arg(
                        Arg::new("output")
                            .short('o')
                            .long("output")
                            .required(true)
                            .value_name("OUTPUT")
                            .help(tr("Output path")),
                    ),
            ))
            .subcommand(decorate(
                Command::new("text")
                    .about(tr("Extract text"))
                    .arg(
                        Arg::new("file")
                            .required(true)
                            .value_name("FILE")
                            .help_heading(arguments)
                            .help(tr("PDF file")),
                    )
                    .arg(
                        Arg::new("page")
                            .long("page")
                            .value_parser(value_parser!(u32))
                            .help(tr("Page number (1-based)")),
                    ),
            ))
            .subcommand(decorate(
                Command::new("merge")
                    .about(tr("Merge two or more PDFs"))
                    .arg(
                        Arg::new("files")
                            .required(true)
                            .num_args(1..)
                            .value_name("FILES")
                            .help_heading(arguments)
                            .help(tr("Input PDF files")),
                    )
                    .arg(
                        Arg::new("output")
                            .short('o')
                            .long("output")
                            .required(true)
                            .value_name("OUTPUT")
                            .help(tr("Output path")),
                    ),
            ))
            .subcommand(decorate(
                Command::new("split")
                    .about(tr(
                        "Split a PDF into one file per page (or per --pages range)",
                    ))
                    .arg(
                        Arg::new("file")
                            .required(true)
                            .value_name("FILE")
                            .help_heading(arguments)
                            .help(tr("PDF file")),
                    )
                    .arg(
                        Arg::new("output")
                            .short('o')
                            .long("output")
                            .required(true)
                            .value_name("OUTPUT")
                            .help(tr("Output path")),
                    )
                    .arg(
                        Arg::new("pages")
                            .long("pages")
                            .help(tr("1-based ranges, e.g. 1-3,5")),
                    ),
            )),
    );
    cmd
}

fn main() {
    init_i18n();
    if let Err(e) = run() {
        eprintln!(
            "{}",
            tr_args("openpdf: {error}", &[("error", &format!("{e:#}"))])
        );
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let matches = cli().get_matches();
    match matches.subcommand() {
        Some(("info", m)) => {
            let file = path_arg(m, "file");
            info(&file)
        }
        Some(("render", m)) => {
            let file = path_arg(m, "file");
            let page = *m.get_one::<u32>("page").unwrap();
            let zoom = *m.get_one::<f32>("zoom").unwrap();
            let output = path_arg(m, "output");
            render(&file, page, zoom, &output)
        }
        Some(("text", m)) => {
            let file = path_arg(m, "file");
            let page = m.get_one::<u32>("page").copied();
            text(&file, page)
        }
        Some(("merge", m)) => {
            let files: Vec<PathBuf> = m
                .get_many::<String>("files")
                .unwrap()
                .map(PathBuf::from)
                .collect();
            let output = path_arg(m, "output");
            merge(&files, &output)
        }
        Some(("split", m)) => {
            let file = path_arg(m, "file");
            let output = path_arg(m, "output");
            let pages = m.get_one::<String>("pages").map(String::as_str);
            split_cmd(&file, &output, pages)
        }
        _ => unreachable!(),
    }
}

fn path_arg(matches: &clap::ArgMatches, id: &str) -> PathBuf {
    PathBuf::from(matches.get_one::<String>(id).unwrap())
}

fn info(path: &Path) -> Result<()> {
    let doc = Document::open(path).with_context(|| {
        tr_args("open {path}", &[("path", &path.display().to_string())])
    })?;
    println!(
        "{}",
        tr_args("file: {path}", &[("path", &path.display().to_string())])
    );
    println!(
        "{}",
        tr_args(
            "pages: {count}",
            &[("count", &doc.page_count().to_string())]
        )
    );
    for page in doc.pages() {
        println!(
            "{}",
            tr_args(
                "page {n}: {w} x {h} pt  rot {rot}",
                &[
                    ("n", &(page.index + 1).to_string()),
                    ("w", &format!("{:.1}", page.width_pt)),
                    ("h", &format!("{:.1}", page.height_pt)),
                    ("rot", &page.rotation.to_string()),
                ],
            )
        );
    }
    let outline = doc.outline().context(tr("read outline"))?;
    if outline.is_empty() {
        println!("{}", tr("outline: (none)"));
    } else {
        println!("{}", tr("outline:"));
        print_outline(&outline, 0);
    }
    Ok(())
}

fn print_outline(items: &[openpdf_core::OutlineItem], depth: usize) {
    for item in items {
        let pad = "  ".repeat(depth + 1);
        let title = tr(&item.title);
        match item.page {
            Some(p) => println!(
                "{pad}{}",
                tr_args(
                    "{title} -> {page}",
                    &[("title", &title), ("page", &(p + 1).to_string())]
                )
            ),
            None => println!("{pad}{title}"),
        }
        print_outline(&item.children, depth + 1);
    }
}

fn engine() -> Result<Engine> {
    Engine::new().context(tr(
        "load libpdfium (set OPENPDF_PDFIUM or install it on the system)",
    ))
}

fn render(path: &Path, page_1: u32, zoom: f32, output: &Path) -> Result<()> {
    if page_1 == 0 {
        bail!("{}", tr("--page is 1-based"));
    }
    let engine = engine()?;
    let doc = engine.open(path)?;
    let raster = engine.render_page(doc.id(), page_1 - 1, zoom)?;
    let img = ImageBuffer::<Rgba<u8>, _>::from_raw(raster.width, raster.height, raster.rgba)
        .context(tr("RGBA buffer size"))?;
    img.save(output).with_context(|| {
        tr_args("write {path}", &[("path", &output.display().to_string())])
    })?;
    println!(
        "{}",
        tr_args("wrote {path}", &[("path", &output.display().to_string())])
    );
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
        Some(0) => bail!("{}", tr("--page is 1-based")),
        Some(p) => print!("{}", engine.page_text(doc.id(), p - 1)?),
    }
    Ok(())
}

fn merge(files: &[PathBuf], output: &Path) -> Result<()> {
    if files.len() < 2 {
        bail!("{}", tr("merge needs at least two input files and -o"));
    }
    let mut doc = merge_paths(files)?;
    doc.save(output)?;
    println!(
        "{}",
        tr_args(
            "wrote {path} ({count} pages)",
            &[
                ("path", &output.display().to_string()),
                ("count", &doc.page_count().to_string()),
            ],
        )
    );
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
        println!(
            "{}",
            tr_args("wrote {path}", &[("path", &output.display().to_string())])
        );
        return Ok(());
    }

    let dir = output;
    fs::create_dir_all(dir)?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    for (i, mut part) in parts.into_iter().enumerate() {
        let dest = dir.join(format!("{}-{}.pdf", stem, i + 1));
        part.save(&dest)?;
        println!(
            "{}",
            tr_args("wrote {path}", &[("path", &dest.display().to_string())])
        );
    }
    Ok(())
}

fn looks_like_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}
