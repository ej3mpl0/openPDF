use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{value_parser, Arg, ArgMatches, Command};
use openpdf_core::{
    apply_overlay, compare_pdfs, crop_pages, extract, images_to_pdf, insert_pages, ocr,
    ocr_capability, optimize, parse_page_ranges, pdf_to_rasters, protect, raster_to_png, redact,
    replace_pages, sanitize_metadata, tr, tr_args, unprotect, CropBox, Document, Engine,
    OcrOptions, OptimizeOptions, Overlay, OverlayKind, PageRange, PdfPermissions, ProtectOptions,
    RedactBox,
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
                Arg::new("user")
                    .long("user")
                    .required(true)
                    .help(tr("User password")),
            )
            .arg(
                Arg::new("owner")
                    .long("owner")
                    .help(tr("Owner password (defaults to user)")),
            )
            .arg(Arg::new("permit").long("permit").help(tr(
                "Permissions: print,modify,copy,annotate,fill,assemble,print-high",
            )))
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("unprotect")
            .about(tr("Remove password protection"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("password")
                    .long("password")
                    .required(true)
                    .help(tr("User or owner password")),
            )
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
    .subcommand(decorate(
        Command::new("ocr")
            .about(tr("Add an invisible searchable text layer (Tesseract)"))
            .arg(file_arg(arguments).required(false))
            .arg(
                Arg::new("lang")
                    .long("lang")
                    .default_value("eng")
                    .help(tr("Tesseract language (eng, spa, …)")),
            )
            .arg(pages_arg())
            .arg(
                Arg::new("list-langs")
                    .long("list-langs")
                    .action(clap::ArgAction::SetTrue)
                    .help(tr("List installed OCR languages")),
            )
            .arg(output_arg().required(false)),
    ))
    .subcommand(decorate(
        Command::new("redact")
            .about(tr("Permanently remove content in a box (true redaction)"))
            .arg(file_arg(arguments))
            .arg(
                Arg::new("box")
                    .long("box")
                    .required(true)
                    .action(clap::ArgAction::Append)
                    .help(tr("Redaction box page,llx,lly,urx,ury (page is 1-based)")),
            )
            .arg(output_arg()),
    ))
    .subcommand(decorate(
        Command::new("compare")
            .about(tr("Compare two PDFs (text + per-page pixels)"))
            .arg(
                Arg::new("left")
                    .required(true)
                    .value_name("LEFT")
                    .help_heading(arguments)
                    .help(tr("First PDF")),
            )
            .arg(
                Arg::new("right")
                    .required(true)
                    .value_name("RIGHT")
                    .help_heading(arguments)
                    .help(tr("Second PDF")),
            )
            .arg(
                Arg::new("zoom")
                    .long("zoom")
                    .value_parser(value_parser!(f32))
                    .default_value("1.0")
                    .help(tr("Zoom factor")),
            ),
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
        Some(("ocr", m)) => ocr_cmd(m),
        Some(("redact", m)) => redact_cmd(m),
        Some(("compare", m)) => compare_cmd(m),
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
    let target = Document::open(path_arg(m, "file"))?;
    let source = Document::open(path_arg(m, "source"))?;
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
    let doc = Document::open(path_arg(m, "file"))?;
    let spec = m.get_one::<String>("pages").unwrap();
    let ranges = parse_page_ranges(spec, doc.page_count())?;
    save_out(extract(&doc, &ranges)?, &path_arg(m, "output"))
}

fn replace_cmd(m: &ArgMatches) -> Result<()> {
    let target = Document::open(path_arg(m, "file"))?;
    let source = Document::open(path_arg(m, "source"))?;
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
    let doc = Document::open(path_arg(m, "file"))?;
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
    let doc = Document::open(path_arg(m, "file"))?;
    let opts = OptimizeOptions {
        jpeg_quality: Some(*m.get_one::<u8>("quality").unwrap()),
        drop_unused: true,
        linearize: !m.get_flag("no-linearize"),
    };
    save_out(optimize(&doc, &opts)?, &path_arg(m, "output"))
}

fn protect_cmd(m: &ArgMatches) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
    let user = m.get_one::<String>("user").unwrap();
    let owner = m
        .get_one::<String>("owner")
        .map(String::as_str)
        .unwrap_or("");
    let mut opts = ProtectOptions::new(user, owner)?;
    if let Some(spec) = m.get_one::<String>("permit") {
        opts.permissions = PdfPermissions::parse_list(spec)?;
    }
    save_out(protect(&doc, &opts)?, &path_arg(m, "output"))
}

fn unprotect_cmd(m: &ArgMatches) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
    let password = m.get_one::<String>("password").unwrap();
    save_out(unprotect(&doc, password)?, &path_arg(m, "output"))
}

fn sanitize_cmd(m: &ArgMatches) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
    save_out(sanitize_metadata(&doc)?, &path_arg(m, "output"))
}

fn overlay_cmd(m: &ArgMatches, kind: OverlayKind) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
    let mut overlay = Overlay::new(kind, m.get_one::<String>("text").unwrap())?;
    overlay.pages = ranges_or_all(&doc, m.get_one::<String>("pages").map(String::as_str))?;
    save_out(apply_overlay(&doc, &overlay)?, &path_arg(m, "output"))
}

fn number_cmd(m: &ArgMatches) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
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
    let doc = engine.open(&path)?;
    let rasters = pdf_to_rasters(&engine, &doc, zoom)?;
    if rasters.len() == 1 && looks_png(&output) {
        let png = raster_to_png(&rasters[0])?;
        fs::write(&output, png)
            .with_context(|| tr_args("write {path}", &[("path", &output.display().to_string())]))?;
        println!(
            "{}",
            tr_args("wrote {path}", &[("path", &output.display().to_string())])
        );
        return Ok(());
    }
    fs::create_dir_all(&output)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
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

fn ocr_cmd(m: &ArgMatches) -> Result<()> {
    if m.get_flag("list-langs") {
        let cap = ocr_capability();
        if !cap.available {
            bail!(
                "{}",
                tr("OCR is unavailable (install tesseract-ocr or set OPENPDF_TESSERACT to the binary or libtesseract)")
            );
        }
        for lang in cap.languages {
            println!("{lang}");
        }
        return Ok(());
    }
    let output = m.get_one::<String>("output").map(PathBuf::from);
    let Some(output) = output else {
        bail!("{}", tr("Output path"));
    };
    if m.get_one::<String>("file").is_none() {
        bail!("{}", tr("PDF file"));
    }
    let engine = engine()?;
    let doc = engine.open(path_arg(m, "file"))?;
    let mut opts = OcrOptions {
        language: m.get_one::<String>("lang").unwrap().clone(),
        pages: Vec::new(),
    };
    opts.pages = ranges_or_all(&doc, m.get_one::<String>("pages").map(String::as_str))?;
    save_out(ocr(&engine, &doc, &opts)?, &output)
}

fn redact_cmd(m: &ArgMatches) -> Result<()> {
    let doc = Document::open(path_arg(m, "file"))?;
    let mut boxes = Vec::new();
    for spec in m.get_many::<String>("box").unwrap() {
        boxes.push(parse_redact_box(spec, doc.page_count())?);
    }
    save_out(redact(&doc, &boxes)?, &path_arg(m, "output"))
}

fn parse_redact_box(spec: &str, page_count: u32) -> Result<RedactBox> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    if parts.len() != 5 {
        bail!("{}", tr("redaction box must be page,llx,lly,urx,ury"));
    }
    let page_1: u32 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("{}", tr("page numbers are 1-based")))?;
    if page_1 == 0 {
        bail!("{}", tr("page numbers are 1-based"));
    }
    if page_1 > page_count {
        bail!(
            "{}",
            tr_args(
                "page {page} is out of range (0..{count})",
                &[
                    ("page", &(page_1 - 1).to_string()),
                    ("count", &page_count.to_string()),
                ],
            )
        );
    }
    let nums: Result<Vec<f32>, _> = parts[1..].iter().map(|s| s.parse()).collect();
    let nums =
        nums.map_err(|_| anyhow::anyhow!("{}", tr("redaction box must be page,llx,lly,urx,ury")))?;
    Ok(RedactBox::new(
        page_1 - 1,
        nums[0],
        nums[1],
        nums[2],
        nums[3],
    )?)
}

fn compare_cmd(m: &ArgMatches) -> Result<()> {
    let zoom = *m.get_one::<f32>("zoom").unwrap();
    let engine = engine()?;
    let left = engine.open(path_arg(m, "left"))?;
    let right = engine.open(path_arg(m, "right"))?;
    let report = compare_pdfs(&engine, &left, &right, zoom)?;
    println!(
        "{}",
        tr_args(
            "pages: {left} vs {right}",
            &[
                ("left", &report.left_pages.to_string()),
                ("right", &report.right_pages.to_string()),
            ],
        )
    );
    for page in &report.pages {
        let text = if page.text_equal {
            tr("text equal")
        } else {
            tr("text differs")
        };
        let pixels = if page.pixel_equal {
            tr("pixels equal")
        } else {
            tr_args(
                "pixels differ ({changed}/{total})",
                &[
                    ("changed", &page.changed_pixels.to_string()),
                    ("total", &page.total_pixels.to_string()),
                ],
            )
        };
        println!(
            "{}",
            tr_args(
                "page {n}: {text}; {pixels}",
                &[
                    ("n", &(page.page + 1).to_string()),
                    ("text", &text),
                    ("pixels", &pixels),
                ],
            )
        );
    }
    if report.identical {
        println!("{}", tr("identical"));
    } else {
        println!("{}", tr("differ"));
    }
    Ok(())
}

fn looks_png(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
}
