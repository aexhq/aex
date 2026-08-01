//! Deterministic HTML token serialization without readability heuristics.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, CharacterTokens, EndTag, StartTag, TagToken, Token, TokenSink, TokenSinkResult,
    Tokenizer, TokenizerOpts,
};
use url::Url;

/// Serializes HTML into stable Markdown under the fixed launch tag policy.
#[must_use]
pub fn html_to_markdown(html: &str, base: &Url) -> String {
    tokenize(html, SinkMode::Markdown, Some(base)).finish()
}

/// Extracts deterministic collapsed text under the same drop policy.
#[must_use]
pub fn html_to_text(html: &str) -> String {
    tokenize(html, SinkMode::Text, None)
        .finish()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokenize(html: &str, mode: SinkMode, base: Option<&Url>) -> HtmlSink {
    let sink = HtmlSink {
        state: RefCell::new(State {
            output: String::new(),
            mode,
            base: base.cloned(),
            drop_tags: Vec::new(),
            pre_depth: 0,
            lists: Vec::new(),
            links: Vec::new(),
            pending_space: false,
        }),
    };
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let _result = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinkMode {
    Markdown,
    Text,
}

struct HtmlSink {
    state: RefCell<State>,
}

struct State {
    output: String,
    mode: SinkMode,
    base: Option<Url>,
    drop_tags: Vec<String>,
    pre_depth: usize,
    lists: Vec<bool>,
    links: Vec<Option<String>>,
    pending_space: bool,
}

impl HtmlSink {
    fn finish(self) -> String {
        let mut output = self.state.into_inner().output;
        while output.contains("\n\n\n") {
            output = output.replace("\n\n\n", "\n\n");
        }
        output
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    }
}

impl TokenSink for HtmlSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<Self::Handle> {
        let mut state = self.state.borrow_mut();
        match token {
            CharacterTokens(text) => state.text(&text),
            TagToken(tag) => match tag.kind {
                StartTag => state.start_tag(&tag),
                EndTag => state.end_tag(tag.name.as_ref()),
            },
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

impl State {
    fn text(&mut self, text: &str) {
        if !self.drop_tags.is_empty() {
            return;
        }
        if self.pre_depth > 0 && self.mode == SinkMode::Markdown {
            self.output.push_str(text);
            return;
        }
        for character in text.chars() {
            if character.is_whitespace() {
                self.pending_space = !self.output.is_empty();
            } else {
                if self.pending_space
                    && !self.output.ends_with([' ', '\n'])
                    && !self.output.ends_with("`\n")
                {
                    self.output.push(' ');
                }
                self.pending_space = false;
                self.output.push(character);
            }
        }
    }

    fn start_tag(&mut self, tag: &html5ever::tokenizer::Tag) {
        let name = tag.name.as_ref();
        if is_dropped(name) {
            self.drop_tags.push(name.to_owned());
            return;
        }
        if !self.drop_tags.is_empty() {
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block();
                if self.mode == SinkMode::Markdown {
                    let level = usize::from(name.as_bytes()[1] - b'0');
                    self.output.push_str(&"#".repeat(level));
                    self.output.push(' ');
                }
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" => {
                self.block();
            }
            "br" => self.line(),
            "ul" => self.lists.push(false),
            "ol" => self.lists.push(true),
            "li" => {
                self.line();
                if self.mode == SinkMode::Markdown {
                    self.output
                        .push_str(&"  ".repeat(self.lists.len().saturating_sub(1)));
                    self.output.push_str(if self.lists.last() == Some(&true) {
                        "1. "
                    } else {
                        "- "
                    });
                }
            }
            "pre" => {
                self.block();
                self.pre_depth += 1;
                if self.mode == SinkMode::Markdown {
                    self.output.push_str("```\n");
                }
            }
            "code" if self.pre_depth == 0 && self.mode == SinkMode::Markdown => {
                self.output.push('`');
            }
            "blockquote" => {
                self.block();
                if self.mode == SinkMode::Markdown {
                    self.output.push_str("> ");
                }
            }
            "a" => {
                if self.pending_space && !self.output.ends_with([' ', '\n']) {
                    self.output.push(' ');
                }
                self.pending_space = false;
                let link = tag
                    .attrs
                    .iter()
                    .find(|attribute| attribute.name.local.as_ref() == "href")
                    .and_then(|attribute| self.absolute_link(attribute.value.as_ref()));
                self.links.push(link);
                if self.mode == SinkMode::Markdown {
                    self.output.push('[');
                }
            }
            "table" => self.block(),
            "tr" => {
                self.line();
                if self.mode == SinkMode::Markdown {
                    self.output.push_str("| ");
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, name: &str) {
        if let Some(dropped) = self.drop_tags.last() {
            if dropped == name {
                self.drop_tags.pop();
            }
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article"
            | "header" | "footer" | "main" | "blockquote" | "table" => self.block(),
            "li" | "tr" => self.line(),
            "ul" | "ol" => {
                self.lists.pop();
                self.block();
            }
            "pre" => {
                self.pre_depth = self.pre_depth.saturating_sub(1);
                if self.mode == SinkMode::Markdown {
                    if !self.output.ends_with('\n') {
                        self.output.push('\n');
                    }
                    self.output.push_str("```");
                }
                self.block();
            }
            "code" if self.pre_depth == 0 && self.mode == SinkMode::Markdown => {
                self.output.push('`');
            }
            "a" => {
                let link = self.links.pop().flatten();
                if self.mode == SinkMode::Markdown {
                    self.output.push(']');
                    if let Some(link) = link {
                        self.output.push('(');
                        self.output.push_str(&link);
                        self.output.push(')');
                    }
                }
            }
            "th" | "td" if self.mode == SinkMode::Markdown => self.output.push_str(" | "),
            "th" | "td" => self.pending_space = true,
            _ => {}
        }
    }

    fn absolute_link(&self, link: &str) -> Option<String> {
        self.base
            .as_ref()
            .and_then(|base| base.join(link).ok())
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(Into::into)
    }

    fn line(&mut self) {
        self.pending_space = false;
        while self.output.ends_with(' ') {
            self.output.pop();
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
    }

    fn block(&mut self) {
        self.line();
        if !self.output.is_empty() && !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
    }
}

fn is_dropped(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "svg" | "head" | "noscript" | "template" | "iframe"
    )
}
