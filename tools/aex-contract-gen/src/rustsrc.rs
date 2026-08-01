//! A `rustfmt` fixed-point source builder.
//!
//! Generated Rust is committed and `cargo fmt --all` runs over it like any other
//! source, so the renderer has to emit exactly what `rustfmt` would. The two
//! width constants below mirror the only two `rustfmt` heuristics the shapes in
//! this crate can trip, the same approach `aex-telemetry-schema` already uses.

/// `rustfmt`'s `max_width`, as pinned in `rustfmt.toml`.
pub const MAX_WIDTH: usize = 100;

/// `rustfmt`'s `array_width` under the default small-heuristics profile.
///
/// An array whose comma-separated contents are wider than this is laid out one
/// element per line, whatever the total line width.
pub const ARRAY_WIDTH: usize = 60;

/// A growing Rust source file.
#[derive(Debug, Default)]
pub struct Source {
    /// The text so far.
    text: String,
}

impl Source {
    /// A new file with the generated-source header.
    #[must_use]
    pub fn new(purpose: &str, digest: &str) -> Self {
        let mut text = String::with_capacity(16 * 1024);
        text.push_str("//! GENERATED — DO NOT EDIT.\n//!\n");
        text.push_str(&format!("//! {purpose}\n//!\n"));
        text.push_str("//! Produced by `aex-contract-gen` from `api/`; contract digest\n");
        text.push_str(&format!("//! `{digest}`.\n"));
        text.push_str("//! Regenerate with `cargo run -p aex-contract-gen -- build`.\n\n");
        // A registry table has one arm per row by construction. Collapsing two
        // arms that happen to share a value today would hide the row, which is
        // the opposite of what an auditable table is for.
        text.push_str(
            "#![allow(clippy::large_enum_variant, reason = \"a wire union is never boxed\")]\n",
        );
        text.push_str("#![allow(clippy::match_same_arms, reason = \"one arm per row\")]\n");
        text.push_str("#![allow(clippy::too_many_lines, reason = \"one arm per row\")]\n\n");
        Self { text }
    }

    /// A new file with no leading module documentation, for an inner section.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// Appends a raw line.
    pub fn line(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push('\n');
    }

    /// Appends a blank line.
    pub fn blank(&mut self) {
        self.text.push('\n');
    }

    /// Appends a documentation comment, wrapped at [`MAX_WIDTH`].
    pub fn doc(&mut self, indent: usize, text: &str) {
        for line in wrap(text, MAX_WIDTH - indent - 4) {
            self.text
                .push_str(&format!("{:indent$}/// {line}\n", "", indent = indent));
        }
    }

    /// Appends an array-valued field the way `rustfmt` lays it out.
    pub fn slice_field(&mut self, indent: usize, field: &str, items: &[String]) {
        let contents = items.join(", ");
        let single = format!("{:indent$}{field}: &[{contents}],", "", indent = indent);
        if contents.len() <= ARRAY_WIDTH && single.len() <= MAX_WIDTH {
            self.line(&single);
            return;
        }
        self.line(&format!("{:indent$}{field}: &[", "", indent = indent));
        let inner = indent + 4;
        for item in items {
            self.line(&format!("{:inner$}{item},", "", inner = inner));
        }
        self.line(&format!("{:indent$}],", "", indent = indent));
    }

    /// Appends one `match` arm the way `rustfmt` lays it out.
    ///
    /// An arm whose single-line form exceeds [`MAX_WIDTH`] is wrapped in a block
    /// and loses its trailing comma, which is exactly what `rustfmt` does.
    pub fn arm(&mut self, indent: usize, variant: &str, value: &str) {
        let single = format!("{:indent$}Self::{variant} => {value},", "", indent = indent);
        if single.len() <= MAX_WIDTH {
            self.line(&single);
            return;
        }
        self.line(&format!(
            "{:indent$}Self::{variant} => {{",
            "",
            indent = indent
        ));
        let inner = indent + 4;
        self.line(&format!("{:inner$}{value}", "", inner = inner));
        self.line(&format!("{:indent$}}}", "", indent = indent));
    }

    /// Appends an array-valued `const` item the way `rustfmt` lays it out.
    pub fn const_slice(&mut self, indent: usize, declaration: &str, items: &[String]) {
        let contents = items.join(", ");
        let single = format!(
            "{:indent$}{declaration} = &[{contents}];",
            "",
            indent = indent
        );
        let inner = indent + 4;
        if contents.len() <= ARRAY_WIDTH && single.len() <= MAX_WIDTH {
            self.line(&single);
            return;
        }
        // `rustfmt` prefers breaking after `=` when the whole array then fits on
        // one continuation line; only past that does it go one element per line.
        let wrapped = format!("{:inner$}&[{contents}];", "", inner = inner);
        if contents.len() <= ARRAY_WIDTH && wrapped.len() <= MAX_WIDTH {
            self.line(&format!("{:indent$}{declaration} =", "", indent = indent));
            self.line(&wrapped);
            return;
        }
        self.line(&format!(
            "{:indent$}{declaration} = &[",
            "",
            indent = indent
        ));
        for item in items {
            self.line(&format!("{:inner$}{item},", "", inner = inner));
        }
        self.line(&format!("{:indent$}];", "", indent = indent));
    }

    /// The finished source. Always ends with exactly one newline.
    #[must_use]
    pub fn finish(self) -> String {
        let mut text = self.text;
        while text.ends_with("\n\n") {
            text.pop();
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }
}

/// Greedy word wrap. Words longer than `width` are kept whole rather than split,
/// because every long word in this crate is an identifier or a digest.
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Escapes a Rust string literal.
#[must_use]
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
