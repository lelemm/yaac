use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::is_cancelled;

pub const MAX_LINE_BYTES: usize = 20_000;
static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

/// Theme keys shared by the two renderers. Unstyled text is implicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeClass {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Constant,
    Variable,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Heading,
    Emphasis,
    Link,
}

impl ScopeClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Function => "function",
            Self::Type => "type",
            Self::Number => "number",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Operator => "operator",
            Self::Punctuation => "punctuation",
            Self::Tag => "tag",
            Self::Attribute => "attribute",
            Self::Heading => "heading",
            Self::Emphasis => "emphasis",
            Self::Link => "link",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// UTF-8 byte range relative to the start of the line.
    pub range: Range<usize>,
    pub class: ScopeClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// UTF-8 byte range in the document, excluding the LF or CRLF terminator.
    pub range: Range<usize>,
    /// Sorted disjoint style ranges. Gaps use the normal foreground colour.
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub name: String,
    /// Language identifier for a fenced code block.
    pub fence: String,
}

#[derive(Debug)]
pub struct Highlighted {
    pub text: String,
    pub language: Option<Language>,
    pub lines: Vec<Line>,
}

impl Highlighted {
    /// Display text for a zero-based line index.
    pub fn line_text(&self, index: usize) -> &str {
        &self.text[self.lines[index].range.clone()]
    }

    pub(crate) fn line_with_ending(&self, index: usize) -> &str {
        let end = self
            .lines
            .get(index + 1)
            .map_or(self.text.len(), |next| next.range.start);
        &self.text[self.lines[index].range.start..end]
    }
}

pub(crate) fn highlight(path: &str, text: String, cancelled: &AtomicBool) -> Option<Highlighted> {
    if is_cancelled(cancelled) {
        return None;
    }
    let syntaxes = SYNTAXES.get_or_init(two_face::syntax::extra_newlines);
    if is_cancelled(cancelled) {
        return None;
    }
    let syntax = detect(syntaxes, path, text.lines().next().unwrap_or_default());
    let language = syntax.map(language);
    let mut highlighter = syntax.map(Highlighter::new);
    let mut lines = Vec::new();
    let mut pos = 0;
    for raw in text.split_inclusive('\n') {
        if is_cancelled(cancelled) {
            return None;
        }
        let len = raw
            .strip_suffix('\n')
            .map_or(raw, |line| line.strip_suffix('\r').unwrap_or(line))
            .len();
        let spans = highlighter
            .as_mut()
            .map_or_else(Vec::new, |worker| worker.line(raw, len, syntaxes));
        lines.push(Line {
            range: pos..pos + len,
            spans,
        });
        pos += raw.len();
    }
    Some(Highlighted {
        text,
        language,
        lines,
    })
}

fn detect<'a>(
    syntaxes: &'a SyntaxSet,
    path: &str,
    first_line: &str,
) -> Option<&'a SyntaxReference> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let by_name = || {
        // Try the whole basename (Dockerfile, .bashrc), then longest compound suffix first.
        std::iter::once(name)
            .chain(name.match_indices('.').map(|(dot, _)| &name[dot + 1..]))
            .filter(|suffix| !suffix.is_empty())
            .find_map(|suffix| syntaxes.find_syntax_by_extension(suffix))
    };
    by_name()
        .or_else(|| {
            if first_line.is_empty() || first_line.len() > MAX_LINE_BYTES {
                return None;
            }
            modeline(first_line)
                .and_then(|token| syntaxes.find_syntax_by_token(token))
                .or_else(|| syntaxes.find_syntax_by_first_line(first_line))
        })
        .filter(|syntax| syntax.name != "Plain Text")
}

fn modeline(line: &str) -> Option<&str> {
    if let Some((_, tail)) = line.split_once("-*-")
        && let Some((header, _)) = tail.split_once("-*-")
    {
        return header
            .split(';')
            .find_map(|field| {
                let (key, value) = field.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case("mode")
                    .then_some(value.trim())
            })
            .or_else(|| (!header.contains(':')).then_some(header.trim()));
    }

    ["vim:", "vi:", "ex:"].into_iter().find_map(|marker| {
        let (_, settings) = line.split_once(marker)?;
        settings
            .split(|c: char| c.is_ascii_whitespace() || c == ':')
            .find_map(|setting| {
                setting
                    .strip_prefix("ft=")
                    .or_else(|| setting.strip_prefix("filetype="))
                    .or_else(|| setting.strip_prefix("syntax="))
            })
    })
}

fn language(syntax: &SyntaxReference) -> Language {
    let scope = syntax.scope.build_string();
    let fence = match scope.as_str() {
        "source.rust" => "rust",
        "source.python" => "python",
        "source.shell" => "sh",
        "source.js" => "javascript",
        "source.ts" => "typescript",
        "text.html.markdown" => "markdown",
        _ => syntax
            .file_extensions
            .iter()
            .find(|ext| {
                ext.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte))
            })
            .map_or_else(|| syntax.name.as_str(), String::as_str),
    };
    Language {
        name: syntax.name.clone(),
        fence: fence.to_owned(),
    }
}

struct Highlighter<'a> {
    syntax: &'a SyntaxReference,
    parse: ParseState,
    stack: ScopeStack,
    classes: HashMap<Scope, Option<ScopeClass>>,
    mapping_key: Scope,
}

impl<'a> Highlighter<'a> {
    fn new(syntax: &'a SyntaxReference) -> Self {
        Self {
            syntax,
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
            classes: HashMap::new(),
            mapping_key: Scope::new("meta.mapping.key.json").expect("valid syntax scope"),
        }
    }

    fn reset(&mut self) {
        self.parse = ParseState::new(self.syntax);
        self.stack = ScopeStack::new();
    }

    fn line(&mut self, raw: &str, len: usize, set: &SyntaxSet) -> Vec<Span> {
        if len > MAX_LINE_BYTES {
            // A skipped line could open or close a multiline construct.
            self.reset();
            return Vec::new();
        }
        let Ok(ops) = self.parse.parse_line(raw, set) else {
            self.reset();
            return Vec::new();
        };
        let mut spans = Vec::new();
        let mut start = 0;
        for (end, op) in ops {
            let end = end.min(len);
            self.push(&mut spans, start..end);
            start = end;
            // Apply even the operations on the newline to preserve multiline syntax.
            if self.stack.apply(&op).is_err() {
                self.reset();
                return Vec::new();
            }
        }
        self.push(&mut spans, start..len);
        spans
    }

    fn push(&mut self, spans: &mut Vec<Span>, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let Some(class) = self.stack.as_slice().iter().rev().find_map(|scope| {
            *self
                .classes
                .entry(*scope)
                .or_insert_with(|| classify(&scope.build_string()))
        }) else {
            return;
        };
        // The JSON grammar uses the same string scope for keys and values.
        let class =
            if class == ScopeClass::String && self.stack.as_slice().contains(&self.mapping_key) {
                ScopeClass::Attribute
            } else {
                class
            };
        if let Some(last) = spans.last_mut()
            && last.range.end == range.start
            && last.class == class
        {
            last.range.end = range.end;
        } else {
            spans.push(Span { range, class });
        }
    }
}

fn classify(scope: &str) -> Option<ScopeClass> {
    use ScopeClass::*;
    let mut atoms = scope.split('.');
    let class = match (
        atoms.next()?,
        atoms.next().unwrap_or_default(),
        atoms.next().unwrap_or_default(),
    ) {
        ("keyword", "operator", _) => Operator,
        ("storage", "type", "rust" | "primitive") => Type,
        ("keyword" | "storage", _, _) => Keyword,
        ("string", _, _) => String,
        ("comment", _, _) => Comment,
        ("support", "type", "property-name") | ("variable", "other", "property") => Attribute,
        ("entity", "name", "tag") => Tag,
        ("entity", "name", "function") | ("support" | "variable", "function", _) => Function,
        ("entity", "name", "label") => Variable,
        ("entity", "name", "class" | "type" | "struct" | "enum" | "interface" | "trait")
        | ("support", "type" | "class", _) => Type,
        ("constant", "numeric", _) => Number,
        ("constant", _, _) | ("support", "constant", _) => Constant,
        ("variable", _, _) => Variable,
        ("punctuation", _, _) => Punctuation,
        ("entity", "other", "attribute-name") => Attribute,
        ("markup", "heading", _) => Heading,
        ("markup", "bold" | "italic" | "underline" | "strikethrough", _) => Emphasis,
        ("markup", "link", _) => Link,
        ("markup", "raw", _) => String,
        _ => return None,
    };
    Some(class)
}
