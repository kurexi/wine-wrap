//! A minimal parser and writer for Valve's KeyValues text format (`.vdf` / `.acf`), which
//! Steam uses for `libraryfolders.vdf` and `appmanifest_*.acf`.

use std::fmt::{self, Write};

/// A KeyValues node: either a string or an ordered list of children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Child by key (KeyValues keys are case-insensitive).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    /// Follow a path of keys.
    pub fn at(&self, path: &[&str]) -> Option<&Vdf> {
        path.iter().try_fold(self, |node, key| node.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: usize,
}

impl Lexer<'_> {
    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            message: message.into(),
        }
    }

    fn next_token(&mut self) -> Result<Option<Token>, ParseError> {
        loop {
            let Some(&c) = self.chars.peek() else {
                return Ok(None);
            };
            match c {
                '\n' => {
                    self.line += 1;
                    self.chars.next();
                }
                c if c.is_whitespace() => {
                    self.chars.next();
                }
                '/' => {
                    self.chars.next();
                    if self.chars.next_if_eq(&'/').is_none() {
                        return Err(self.err("unexpected '/'"));
                    }
                    while self.chars.next_if(|&c| c != '\n').is_some() {}
                }
                '{' => {
                    self.chars.next();
                    return Ok(Some(Token::Open));
                }
                '}' => {
                    self.chars.next();
                    return Ok(Some(Token::Close));
                }
                '"' => {
                    self.chars.next();
                    return self.quoted().map(|s| Some(Token::Str(s)));
                }
                _ => {
                    let mut s = String::new();
                    while let Some(c) = self
                        .chars
                        .next_if(|&c| !c.is_whitespace() && !matches!(c, '{' | '}' | '"'))
                    {
                        s.push(c);
                    }
                    return Ok(Some(Token::Str(s)));
                }
            }
        }
    }

    fn quoted(&mut self) -> Result<String, ParseError> {
        let mut s = String::new();
        loop {
            match self.chars.next() {
                None => return Err(self.err("unterminated string")),
                Some('"') => return Ok(s),
                Some('\\') => match self.chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some(c @ ('\\' | '"')) => s.push(c),
                    // Unknown escapes are kept verbatim, like Steam does.
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                    None => return Err(self.err("unterminated string")),
                },
                Some(c) => {
                    if c == '\n' {
                        self.line += 1;
                    }
                    s.push(c);
                }
            }
        }
    }
}

/// Parse a KeyValues document. The result is an object holding the top-level
/// key(s), e.g. `{"AppState": {...}}`.
pub fn parse(text: &str) -> Result<Vdf, ParseError> {
    let mut lexer = Lexer {
        chars: text
            .strip_prefix('\u{feff}')
            .unwrap_or(text)
            .chars()
            .peekable(),
        line: 1,
    };
    let items = parse_object(&mut lexer, false)?;
    Ok(Vdf::Obj(items))
}

/// Write a document (as returned by [`parse`]) in Steam's own layout: quoted
/// keys and values, tab indentation.
pub fn write(doc: &Vdf) -> String {
    fn quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
    fn write_items(out: &mut String, items: &[(String, Vdf)], depth: usize) {
        let indent = "\t".repeat(depth);
        for (key, value) in items {
            // Writing to a String can't fail.
            let _ = match value {
                Vdf::Str(v) => writeln!(out, "{indent}{}\t\t{}", quote(key), quote(v)),
                Vdf::Obj(children) => {
                    let _ = writeln!(out, "{indent}{}\n{indent}{{", quote(key));
                    write_items(out, children, depth + 1);
                    writeln!(out, "{indent}}}")
                }
            };
        }
    }
    let mut out = String::new();
    write_items(&mut out, doc.entries(), 0);
    out
}

fn parse_object(lexer: &mut Lexer<'_>, nested: bool) -> Result<Vec<(String, Vdf)>, ParseError> {
    let mut items = Vec::new();
    loop {
        let key = match lexer.next_token()? {
            Some(Token::Str(k)) => k,
            Some(Token::Close) if nested => return Ok(items),
            None if !nested => return Ok(items),
            None => return Err(lexer.err("unexpected end of input, expected '}'")),
            Some(t) => return Err(lexer.err(format!("expected key, found {t:?}"))),
        };
        let value = match lexer.next_token()? {
            Some(Token::Str(v)) => Vdf::Str(v),
            Some(Token::Open) => Vdf::Obj(parse_object(lexer, true)?),
            Some(Token::Close) | None => {
                return Err(lexer.err(format!("missing value for key {key:?}")))
            }
        };
        items.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_app_manifest() {
        let text = r#"
"AppState"
{
	"appid"		"620"
	"name"		"Portal 2"
	"installdir"		"Portal 2"
	// comment
	"UserConfig"
	{
		"language"		"english"
	}
}
"#;
        let doc = parse(text).unwrap();
        assert_eq!(
            doc.at(&["AppState", "appid"]).and_then(Vdf::as_str),
            Some("620")
        );
        assert_eq!(
            doc.at(&["appstate", "NAME"]).and_then(Vdf::as_str),
            Some("Portal 2")
        );
        assert_eq!(
            doc.at(&["AppState", "UserConfig", "language"])
                .and_then(Vdf::as_str),
            Some("english")
        );
    }

    #[test]
    fn escapes_and_unquoted() {
        let doc = parse(r#"root { path "C:\\Program Files (x86)\\Steam" bare value "q" "a\"b" }"#)
            .unwrap();
        let root = doc.get("root").unwrap();
        assert_eq!(
            root.get("path").and_then(Vdf::as_str),
            Some(r"C:\Program Files (x86)\Steam")
        );
        assert_eq!(root.get("bare").and_then(Vdf::as_str), Some("value"));
        assert_eq!(root.get("q").and_then(Vdf::as_str), Some("a\"b"));
    }

    #[test]
    fn writes_what_it_parses() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\
                    \t\t\"label\"\t\t\"say \\\"hi\\\"\"\n\t\t\"apps\"\n\t\t{\n\t\t}\n\t}\n}\n";
        let doc = parse(text).unwrap();
        assert_eq!(
            doc.at(&["libraryfolders", "0", "path"])
                .and_then(Vdf::as_str),
            Some(r"C:\Program Files (x86)\Steam")
        );
        assert_eq!(write(&doc), text);
    }

    #[test]
    fn errors() {
        assert!(parse(r#""a" { "b" "c" "#).is_err());
        assert!(parse(r#""a" "#).is_err());
        assert!(parse(r#""a" "unterminated"#).is_err());
        let err = parse("\"a\"\n{\n}\n}").unwrap_err();
        assert_eq!(err.line, 4);
    }
}
