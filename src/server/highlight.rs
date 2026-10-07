//! Server-side syntax highlighting for code excerpts.

use std::path::Path;
use std::sync::LazyLock;

use maud::{html, Markup, PreEscaped};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// `text` as a line-numbered, highlighted table starting at `first_line`.
pub fn code_block(path: &str, text: &str, first_line: u32) -> Markup {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
    let syntax = SYNTAXES.find_syntax_by_extension(ext).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, &THEMES.themes["InspiredGitHub"]);
    let rows: Vec<(u32, String)> = LinesWithEndings::from(text)
        .enumerate()
        .map(|(i, line)| {
            let highlighted = h
                .highlight_line(line, &SYNTAXES)
                .ok()
                .and_then(|regions| styled_line_to_highlighted_html(&regions, IncludeBackground::No).ok())
                .unwrap_or_else(|| html! { (line) }.into_string());
            (first_line + i as u32, highlighted)
        })
        .collect();
    html! {
        table.code {
            @for (n, line) in &rows {
                tr { td.ln { (n) } td.src { (PreEscaped(line)) } }
            }
        }
    }
}
