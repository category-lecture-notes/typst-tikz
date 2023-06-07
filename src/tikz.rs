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

const REGEX_PATTERN_TIKZ: &str = r"(?P<environment>tikzpicture|tikzcd)\[(?P<tex_code>[^\[\]]*(?:\[[^\[\]]*\][^\[\]]*)*)\]";

const LATEX_ENGINE: &str = "lualatex";
const LATEX_DOCUMENT_BEGIN: &str = r#"
    \documentclass[tikz]{standalone}
    \usepackage{tikz-cd}

    \begin{document}
"#;
const LATEX_DOCUMENT_END: &str = r#"
    \end{document}
"#;

const LUA_CONFIG: &str = r#"
    texconfig.file_line_error = true
    texconfig.halt_on_error = true
    texconfig.interaction = 1

    callback.register('show_error_message', function(...)
        texio.write_nl('term and log', status.lasterrorstring)
        texio.write('term', '.\n')
    end)

    callback.register('show_lua_error_hook', function(...)
        texio.write_nl('term and log', status.lastluaerrorstring)
        texio.write('term', '.\n')
    end)
"#;

pub struct Tikz {
    tempdir: TempDir,
    images: FrozenMap<u64, Box<Result<Vec<u8>, String>>>,
}

fn execute(cmd: &mut Command) -> Result<(), String> {
    let child = cmd.stdout(Stdio::piped()).spawn().map_err(|err| err.to_string())?;
    let Output { status, stdout, .. } =
        child.wait_with_output().map_err(|err| err.to_string())?;

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

    pub fn fetch(&self, name: &str) -> &Result<Vec<u8>, String> {
        const PREFIX_SIZE: usize = "generated_tikz_".len();
        const SUFFIX_SIZE: usize = ".svg".len();

        let index = name[PREFIX_SIZE..name.len() - SUFFIX_SIZE].parse::<u64>().unwrap();

        self.images.get(&index).unwrap()
    }

    pub fn replace(&self, buffer: &str) -> String {
        lazy_static! {
            static ref REG_TIKZ: Regex = Regex::new(REGEX_PATTERN_TIKZ).unwrap();
        }

        let mut images = VecDeque::new();

        for capture in REG_TIKZ.captures_iter(buffer) {
            let environment = capture.name("environment").unwrap().as_str();
            let tex_code = capture.name("tex_code").unwrap().as_str();

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
                images.push_back(format!(r#"image("generated_tikz_{}.svg")"#, hash));
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
                r#"image("generated_tikz_{}.svg", width: {})"#,
                hash, width
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

        let mut file = File::create(&tex_path).map_err(|err| err.to_string())?;
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

        read(&svg_path).map_err(|err| err.to_string())
    }
}
