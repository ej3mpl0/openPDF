use std::fs;
use std::path::{Path, PathBuf};

use crate::password::{read_optional_password, read_password};
use anyhow::{bail, Context, Result};
use clap::{value_parser, Arg, ArgMatches, Command};
use openpdf_core::{
    apply_overlay, crop_pages, extract, images_to_pdf, insert_pages, optimize, parse_page_ranges,
    pdf_to_rasters, protect, raster_to_png, replace_pages, sanitize_metadata, tr, tr_args,
    unprotect, CropBox, Document, Engine, OptimizeOptions, Overlay, OverlayKind, PageRange,
    PdfPermissions, ProtectOptions,
};

use crate::{decorate, engine, path_arg};

pub fn register(cmd: Command, arguments: &'static str) -> Command {
    cmd.subcommand(decorate(
        Command::new("insert")
            .about(tr("Insert pages from another PDF"))
            .arg(file_arg(arguments))
            .arg(source_arg(arguments))
            .arg(
                Arg::new("at")
                    .long("at")
                    .value_parser(value_parser!(u32))
                    .required(true)
                    .help(tr("1-based insert position")),
            )
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("extract")
            .about(tr("Extract pages to a new PDF"))
            .arg(file_arg(arguments))
            .arg(pages_arg().required(true))
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("replace")
            .about(tr("Replace pages with pages from another PDF"))
            .arg(file_arg(arguments))
            .arg(source_arg(arguments))
            .arg(
                Arg::new("at")
                    .long("at")
                    .value_parser(value_parser!(u32))
                    .required(true)
                    .help(tr("1-based start page to replace")),
            )
            .arg(
                Arg::new("count")
                    .long("count")
                    .value_parser(value_parser!(u32))
                    .default_value("1")
                    .help(tr("Number of pages to replace")),
            )
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("crop")
            .about(tr("Set a crop box on pages"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("box")
                    .long("box")
                    .required(true)
                    .help(tr("Crop box llx,lly,urx,ury in points")),
            )
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("optimize")
            .about(tr("Recompress images, drop unused objects, linearize"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("quality")
                    .long("quality")
                    .value_parser(value_parser!(u8))
                    .default_value("75")
                    .help(tr("JPEG quality 1-100")),
            )
            .arg(
                Arg::new("no-linearize")
                    .long("no-linearize")
                    .action(clap::ArgAction::SetTrue)
                    .help(tr("Skip the linearized hint")),
            )
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("protect")
            .about(tr("Encrypt with a user/owner password (AES-256)"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("permit")
                    .long("permit")
                    .help(tr("Permissions: print,modify,copy,annotate,fill,assemble,print-high")),
            )
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("unprotect")
            .about(tr("Remove password protection"))
            .arg(file_arg(arguments))
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("sanitize")
            .about(tr("Remove metadata, JavaScript, and embedded-file names"))
            .arg(file_arg(arguments))
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("watermark")
            .about(tr("Stamp a watermark"))
            .arg(file_arg(arguments))
            .arg(text_arg())
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("header")
            .about(tr("Add a header"))
            .arg(file_arg(arguments))
            .arg(text_arg())
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("footer")
            .about(tr("Add a footer"))
            .arg(file_arg(arguments))
            .arg(text_arg())
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("number")
            .about(tr("Add page numbers"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("format")
                    .long("format")
                    .default_value("{page} / {total}")
                    .help(tr("Number format; {page} and {total} are placeholders")),
            )
            .arg(pages_arg())
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("from-images")
            .about(tr("Build a PDF from images"))
            .arg(
                Arg::new("images")
                    .required(true)
                    .num_args(1..)
                    .value_name("IMAGES")
                    .help_heading(arguments)
                    .help(tr("Image files")),
            )
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("to-images")
            .about(tr("Render pages to PNG files"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("zoom")
                    .long("zoom")
                    .value_parser(value_parser!(f32))
                    .default_value("1.0")
                    .help(tr("Zoom factor")),
            )
            .arg(output_arg()),
    ))
}

pub fn dispatch(matches: &ArgMatches) -> Option<Result<()>> {
    Some(match matches.subcommand() {
        Some(("insert", m)) => insert_cmd(m),
        Some(("extract", m)) => extract_cmd(m),
        Some(("replace", m)) => replace_cmd(m),
        Some(("crop", m)) => crop_cmd(m),
        Some(("optimize", m)) => optimize_cmd(m),
        Some(("protect", m)) => protect_cmd(m),
        Some(("unprotect", m)) => unprotect_cmd(m),
        Some(("sanitize", m)) => sanitize_cmd(m),
        Some(("watermark", m)) => overlay_cmd(m, OverlayKind::Watermark),
        Some(("header", m)) => overlay_cmd(m, OverlayKind::Header),
        Some(("footer", m)) => overlay_cmd(m, OverlayKind::Footer),
        Some(("number", m)) => number_cmd(m),
        Some(("from-images", m)) => from_images_cmd(m),
        Some(("to-images", m)) => to_images_cmd(m),
        _ => return None,
    })
}

fn file_arg(arguments: &'static str) -> Arg {
    Arg::new("file")
        .required(true)
        .value_name("FILE")
        .help_heading(arguments)
        .help(tr("PDF file"))
}

fn source_arg(arguments: &'static str) -> Arg {
    Arg::new("source")
        .required(true)
        .value_name("SOURCE")
        .help_heading(arguments)
        .help(tr("Source PDF"))
}

fn output_arg() -> Arg {
    Arg::new("output")
        .short('o')
        .long("output")
        .required(true)
        .value_name("OUTPUT")
        .help(tr("Output path"))
}

fn pages_arg() -> Arg {
    Arg::new("pages")
        .long("pages")
        .help(tr("1-based ranges, e.g. 1-3,5"))
}

fn text_arg() -> Arg {
    Arg::new("text")
        .long("text")
        .required(true)
        .help(tr("Overlay text"))
}

fn ranges_or_all(doc: &Document, spec: Option<&str>) -> Result<Vec<PageRange>> {
    match spec {
        Some(s) => Ok(parse_page_ranges(s, doc.page_count())?),
        None => Ok((0..doc.page_count()).map(PageRange::single).collect()),
    }
}

fn save_out(mut doc: Document, output: &Path) -> Result<()> {
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

fn insert_cmd(m: &ArgMatches) -> Result<()> {
    let target = crate::password::open(path_arg(m, "file"))?;
    let source = crate::password::open(path_arg(m, "source"))?;
    let at_1 = *m.get_one::<u32>("at").unwrap();
    if at_1 == 0 {
        bail!("{}", tr("--at is 1-based"));
    }
    let spec = m
        .get_one::<String>("pages")
        .map(String::as_str)
        .unwrap_or("1");
    let ranges = parse_page_ranges(spec, source.page_count())?;
    let out = insert_pages(&target, at_1 - 1, &source, &ranges)?;
    save_out(out, &path_arg(m, "output"))
}

fn extract_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    let spec = m.get_one::<String>("pages").unwrap();
    let ranges = parse_page_ranges(spec, doc.page_count())?;
    save_out(extract(&doc, &ranges)?, &path_arg(m, "output"))
}

fn replace_cmd(m: &ArgMatches) -> Result<()> {
    let target = crate::password::open(path_arg(m, "file"))?;
    let source = crate::password::open(path_arg(m, "source"))?;
    let at_1 = *m.get_one::<u32>("at").unwrap();
    if at_1 == 0 {
        bail!("{}", tr("--at is 1-based"));
    }
    let count = *m.get_one::<u32>("count").unwrap();
    let spec = m
        .get_one::<String>("pages")
        .map(String::as_str)
        .unwrap_or("1");
    let ranges = parse_page_ranges(spec, source.page_count())?;
    let out = replace_pages(&target, at_1 - 1, count, &source, &ranges)?;
    save_out(out, &path_arg(m, "output"))
}

fn crop_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    let spec = m.get_one::<String>("box").unwrap();
    let nums: Result<Vec<f32>, _> = spec.split(',').map(|s| s.trim().parse()).collect();
    let nums = nums.context(tr("crop box must be llx,lly,urx,ury"))?;
    if nums.len() != 4 {
        bail!("{}", tr("crop box must be llx,lly,urx,ury"));
    }
    let crop = CropBox::new(nums[0], nums[1], nums[2], nums[3])?;
    let pages = m.get_one::<String>("pages").map(String::as_str);
    let ranges = if let Some(s) = pages {
        parse_page_ranges(s, doc.page_count())?
    } else {
        Vec::new()
    };
    save_out(crop_pages(&doc, &ranges, crop)?, &path_arg(m, "output"))
}

fn optimize_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    let opts = OptimizeOptions {
        jpeg_quality: Some(*m.get_one::<u8>("quality").unwrap()),
        drop_unused: true,
        linearize: !m.get_flag("no-linearize"),
    };
    save_out(optimize(&doc, &opts)?, &path_arg(m, "output"))
}

fn protect_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    // Stdin: line 1 user password, optional line 2 owner password.
    let user = read_password(&tr("User password: "), 0)?;
    let owner = read_optional_password(&tr("Owner password (empty = same as user): "), 1)?;
    let mut opts = ProtectOptions::new(
        user.as_str(),
        owner.as_ref().map(|o| o.as_str()).unwrap_or(""),
    )?;
    if let Some(spec) = m.get_one::<String>("permit") {
        opts.permissions = PdfPermissions::parse_list(spec)?;
    }
    save_out(protect(&doc, &opts)?, &path_arg(m, "output"))
}

fn unprotect_cmd(m: &ArgMatches) -> Result<()> {
    let (doc, password) = crate::password::open_doc(&path_arg(m, "file"))?;
    // An unencrypted file never asked for a password; core reports NotEncrypted.
    let password = password.as_ref().map(|p| p.as_str()).unwrap_or("");
    save_out(unprotect(&doc, password)?, &path_arg(m, "output"))
}

fn sanitize_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    save_out(sanitize_metadata(&doc)?, &path_arg(m, "output"))
}

fn overlay_cmd(m: &ArgMatches, kind: OverlayKind) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    let mut overlay = Overlay::new(kind, m.get_one::<String>("text").unwrap())?;
    overlay.pages = ranges_or_all(&doc, m.get_one::<String>("pages").map(String::as_str))?;
    save_out(apply_overlay(&doc, &overlay)?, &path_arg(m, "output"))
}

fn number_cmd(m: &ArgMatches) -> Result<()> {
    let doc = crate::password::open(path_arg(m, "file"))?;
    let format = m
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("{page} / {total}");
    let mut overlay = Overlay::new(OverlayKind::PageNumber, format)?;
    overlay.pages = ranges_or_all(&doc, m.get_one::<String>("pages").map(String::as_str))?;
    save_out(apply_overlay(&doc, &overlay)?, &path_arg(m, "output"))
}

fn from_images_cmd(m: &ArgMatches) -> Result<()> {
    let images: Vec<PathBuf> = m
        .get_many::<String>("images")
        .unwrap()
        .map(PathBuf::from)
        .collect();
    save_out(images_to_pdf(&images)?, &path_arg(m, "output"))
}

fn to_images_cmd(m: &ArgMatches) -> Result<()> {
    let path = path_arg(m, "file");
    let zoom = *m.get_one::<f32>("zoom").unwrap();
    let output = path_arg(m, "output");
    let engine: Engine = engine()?;
    let doc = crate::password::open(&path)?;
    engine.bind(&doc)?;
    let rasters = pdf_to_rasters(&engine, &doc, zoom)?;
    if rasters.len() == 1 && looks_png(&output) {
        let png = raster_to_png(&rasters[0])?;
        fs::write(&output, png).with_context(|| {
            tr_args("write {path}", &[("path", &output.display().to_string())])
        })?;
        println!(
            "{}",
            tr_args("wrote {path}", &[("path", &output.display().to_string())])
        );
        return Ok(());
    }
    fs::create_dir_all(&output)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    for (i, raster) in rasters.iter().enumerate() {
        let dest = output.join(format!("{}-{}.png", stem, i + 1));
        let png = raster_to_png(raster)?;
        fs::write(&dest, png)?;
        println!(
            "{}",
            tr_args("wrote {path}", &[("path", &dest.display().to_string())])
        );
    }
    Ok(())
}

fn looks_png(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}
