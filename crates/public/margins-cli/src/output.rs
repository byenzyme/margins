use crate::error::CliError;
use std::io::{self, Write};

pub fn xml_escape_text(value: &str) -> String {
    value
        .chars()
        .filter_map(xml_char)
        .flat_map(|character| match character {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            _ => vec![character],
        })
        .collect()
}

pub fn xml_escape_attr(value: &str) -> String {
    xml_escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_char(character: char) -> Option<char> {
    (character == '\n' || character == '\r' || character == '\t' || !character.is_control())
        .then_some(character)
}

pub fn write_error(stderr: &mut dyn Write, error: &CliError) -> io::Result<()> {
    writeln!(
        stderr,
        "<margins_error code=\"{}\">{}</margins_error>",
        xml_escape_attr(error.code()),
        xml_escape_text(error.message())
    )
}

pub fn line(output: &mut dyn Write, args: std::fmt::Arguments<'_>) -> anyhow::Result<()> {
    output.write_fmt(args)?;
    output.write_all(b"\n")?;
    Ok(())
}
