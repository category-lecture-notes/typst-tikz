use elsa::FrozenMap;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::fs::{read, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Output, Stdio};
use svg_metadata::{Metadata, Unit, Width};
use tempfile::TempDir;
use typst::diag::SourceError;
use typst::World;

const REGEX_PATTERN_TIKZ: &str = r"(?P<environment>tikzpicture|tikzcd)\[(?P<block>\s*```(?P<tex_code>(?s).*?)```\s*)\]";

const LATEX_ENGINE: &str = "lualatex";
const LATEX_DOCUMENT_BEGIN: &str = concat!(
    r#"\documentclass[tikz]{standalone}"#,
    include_str!("../assets/latex/quiver.sty"),
    r#"\begin{document}"#
);

const LATEX_DOCUMENT_END: &str = r#"\end{document}"#;

const LUA_CONFIG: &str = r#"
    texconfig.halt_on_error = true
    texconfig.interaction = 0

    callback.register('show_error_message', function(...)
        texio.write_nl('term and log', status.lasterrorstring)
        texio.write('term', '.\n')
    end)

    callback.register('show_lua_error_hook', function(...)
        texio.write_nl('term and log', status.lastluaerrorstring)
        texio.write('term', '.\n')
    end)
"#;

const PREFIX: &str = "generated_tikz_";
const SUFFIX: &str = ".svg";
const PREFIX_SIZE: usize = PREFIX.len();
const SUFFIX_SIZE: usize = SUFFIX.len();

pub struct Tikz {
    tempdir: TempDir,
    images: FrozenMap<u64, Box<Result<Vec<u8>, String>>>,
}

fn execute(cmd: &mut Command) -> Result<(), String> {
    let child = cmd.stdout(Stdio::piped()).spawn().map_err(|err| {
        format!("failed to invoke {}: {}", cmd.get_program().to_string_lossy(), err)
    })?;

    let Output { status, stdout, .. } = child
        .wait_with_output()
        .map_err(|err| format!("failed to fetch LaTeX process: {}", err))?;

    if !status.success() {
        return Err(String::from_utf8(stdout).unwrap());
    }

    Ok(())
}

impl Tikz {
    pub fn new() -> std::io::Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let config_path = tempdir.path().join("config.lua");

        let mut file = File::create(config_path)?;
        writeln!(file, "{}", LUA_CONFIG)?;

        Ok(Self { tempdir, images: FrozenMap::new() })
    }

    pub fn fetch(&self, index: u64) -> &Result<Vec<u8>, String> {
        self.images.get(&index).unwrap()
    }

    pub fn replace(&self, buffer: &str) -> String {
        lazy_static! {
            static ref REG_TIKZ: Regex = Regex::new(REGEX_PATTERN_TIKZ).unwrap();
        }

        let mut images = VecDeque::new();

        for capture in REG_TIKZ.captures_iter(buffer) {
            let environment = capture.name("environment").unwrap().as_str();
            let block = capture.name("block").unwrap().as_str();
            let tex_code = capture.name("tex_code").unwrap().as_str();

            let lines = "\n".repeat(block.split('\n').count() - 1);

            let mut hasher = DefaultHasher::new();
            environment.hash(&mut hasher);
            tex_code.hash(&mut hasher);

            let hash = hasher.finish();

            let image = match self.images.get(&hash) {
                Some(image) => image,
                None => {
                    let image = Box::new(self.invoke_latex(tex_code, environment));

                    self.images.insert(hash, image);

                    self.images.get(&hash).unwrap()
                }
            };

            let Ok(image) = image else {
                images.push_back(format!(r#"image("{}{}{}"){}"#, PREFIX, hash, SUFFIX, lines));
                continue;
            };

            let svg = std::str::from_utf8(image.as_ref()).unwrap();
            let width = match Metadata::parse(svg).unwrap().width.unwrap() {
                Width { width, unit: Unit::Em } => format!("{}em", width),
                Width { width, unit: Unit::Pt } => format!("{}pt", width),
                Width { width, unit: Unit::Cm } => format!("{}cm", width),
                Width { width, unit: Unit::Mm } => format!("{}mm", width),
                Width { width, unit: Unit::In } => format!("{}in", width),
                Width { width, unit: Unit::Percent } => format!("{}%", width),
                _ => panic!("Unsupported SVG-generated unit"),
            };

            images.push_back(format!(
                r#"image("{}{}{}", width: {}){}"#,
                PREFIX, hash, SUFFIX, width, lines
            ));
        }

        REG_TIKZ
            .replace_all(buffer, |_: &regex::Captures| images.pop_front().unwrap())
            .to_string()
    }

    fn invoke_latex(&self, tex_code: &str, environment: &str) -> Result<Vec<u8>, String> {
        let tex_path = self.tempdir.path().join("tikz.tex");
        let pdf_path = self.tempdir.path().join("tikz.pdf");
        let svg_path = self.tempdir.path().join("tikz.svg");

        let mut file = File::create(&tex_path)
            .map_err(|err| format!("failed to create LaTeX buffer: {}", err))?;
        writeln!(file, "{}", LATEX_DOCUMENT_BEGIN).map_err(|err| err.to_string())?;
        writeln!(file, "\\begin{{{}}}", environment).map_err(|err| err.to_string())?;
        writeln!(file, "{}", tex_code.trim()).map_err(|err| err.to_string())?;
        writeln!(file, "\\end{{{}}}", environment).map_err(|err| err.to_string())?;
        writeln!(file, "{}", LATEX_DOCUMENT_END).map_err(|err| err.to_string())?;

        let mut process = Command::new(LATEX_ENGINE);
        let process_cmd = process
            .args(["-lua", self.tempdir.path().join("config.lua").to_str().unwrap()])
            .args(["-output-directory", self.tempdir.path().to_str().unwrap()])
            .arg("-no-shell-escape")
            .arg(tex_path);

        execute(process_cmd)?;

        let mut process = Command::new("pdf2svg");
        let process_cmd = process.arg(pdf_path).arg(svg_path.clone());

        execute(process_cmd)?;

        read(&svg_path).map_err(|err| format!("failed to read generated SVG: {}", err))
    }

    pub fn is_error(world: &dyn World, error: &SourceError) -> Option<u64> {
        if error.message != "failed to load file" {
            return None;
        }

        let range = error.span.range(world);
        let source = world.source(error.span.id()).unwrap();
        let filename = source.text()[range.start + 1..range.end - 1].to_string();

        Tikz::is_filename(&filename)
    }

    pub fn is_filename(name: &str) -> Option<u64> {
        if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            name[PREFIX_SIZE..name.len() - SUFFIX_SIZE].parse::<u64>().ok()
        } else {
            None
        }
    }
}
