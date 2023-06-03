use elsa::FrozenVec;
use lazy_static::lazy_static;
use regex::Regex;
use std::fs::{read, File};
use std::io::{Error, ErrorKind, Result, Write};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const REGEX_TIKZPICTURE: &str = r"#tikzpicture\[([^\[\]]*(?:\[[^\[\]]*\][^\[\]]*)*)\]";

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
    images: FrozenVec<Vec<u8>>,
}

fn execute(cmd: &mut Command) -> Result<()> {
    let child = cmd.stdout(Stdio::piped()).spawn()?;
    let Output { status, stdout, .. } = child.wait_with_output()?;

    if !status.success() {
        return Err(Error::new(ErrorKind::Other, String::from_utf8(stdout).unwrap()));
    }

    Ok(())
}

impl Tikz {
    pub fn new() -> Result<Self> {
        let tempdir = tempfile::tempdir()?;
        let config_path = tempdir.path().join("config.lua");

        let mut file = File::create(config_path)?;
        writeln!(file, "{}", LUA_CONFIG)?;

        Ok(Self { tempdir, images: FrozenVec::new() })
    }

    pub fn replace(&self, buffer: &mut String) {
        lazy_static! {
            static ref RE: Regex = Regex::new(REGEX_TIKZPICTURE).unwrap();
        }

        let striped_buffer = RE.replace_all(buffer, |capture: &regex::Captures| {
            let chunk = capture.get(1).unwrap().as_str();

            let image = self.query_tikz(chunk, "tikzpicture").unwrap();
            self.images.push(image);

            format!(r#"#image("generated_tikz_{}.svg")"#, self.images.len() - 1)
        });

        *buffer = striped_buffer.to_string();
    }

    pub fn fetch(&self, name: &str) -> Vec<u8> {
        const PREFIX_SIZE: usize = "generated_tikz_".len();
        const SUFFIX_SIZE: usize = ".svg".len();

        let index = name[PREFIX_SIZE..name.len() - SUFFIX_SIZE].parse::<usize>().unwrap();

        self.images.get(index).unwrap().to_vec()
    }

    fn query_tikz(&self, buffer: &str, environment: &str) -> Result<Vec<u8>> {
        let tex_path = self.tempdir.path().join("tikz.tex");
        let pdf_path = self.tempdir.path().join("tikz.pdf");
        let svg_path = self.tempdir.path().join("tikz.svg");

        let mut file = File::create(&tex_path)?;
        writeln!(file, "{}", LATEX_DOCUMENT_BEGIN)?;
        writeln!(file, "\\begin{{{}}}", environment)?;
        writeln!(file, "{}", buffer)?;
        writeln!(file, "\\end{{{}}}", environment)?;
        writeln!(file, "{}", LATEX_DOCUMENT_END)?;

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

        read(&svg_path)
    }
}
