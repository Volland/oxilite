//! The line editor: prompts, history, highlighting, hints and completion.

use super::render::{Mode, BLUE, BOLD, CYAN, DIM, GREEN, MAGENTA, YELLOW};
use super::{Flow, Result, Session, COMMANDS};
use crate::studio::index::SourceIndex;
use crate::studio::lang::{self, CompletionContext, Lang};
use crate::studio::scanner::{scan, Kind};
use lsp_types::{CompletionTextEdit, Position};
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{ColorMode, CompletionType, Config, Context, Editor, Helper};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

const PROMPT: &str = "oxilite> ";
const CONTINUATION: &str = "   ...> ";

pub struct ShellHelper {
    session: Rc<RefCell<Session>>,
    files: FilenameCompleter,
    hinter: HistoryHinter,
    color: bool,
}

impl Helper for ShellHelper {}
impl Validator for ShellHelper {}

impl Hinter for ShellHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        // Multi-line history entries would print below the prompt; hint single lines only.
        self.hinter
            .hint(line, pos, ctx)
            .filter(|h| !h.contains('\n'))
    }
}

fn paint(text: &str, code: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// SPARQL coloured by token kind; comments dimmed.
pub fn highlight_sparql(line: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    let mut at = 0;
    let gap = |text: &str, out: &mut String| match text.find('#') {
        Some(i) => {
            out.push_str(&text[..i]);
            out.push_str(&paint(&text[i..], DIM));
        }
        None => out.push_str(text),
    };
    for t in scan(line, None).tokens {
        if t.from < at || t.to > line.len() {
            continue;
        }
        gap(&line[at..t.from], &mut out);
        let text = &line[t.from..t.to];
        let code = match &t.kind {
            Kind::Iri(_) | Kind::PName { .. } => BLUE,
            Kind::Var(_) => YELLOW,
            Kind::Str | Kind::At(_) => GREEN,
            Kind::Number => CYAN,
            Kind::BNode => MAGENTA,
            Kind::Word(w) if w == "a" => MAGENTA,
            Kind::Word(w) if w.eq_ignore_ascii_case("true") || w.eq_ignore_ascii_case("false") => {
                CYAN
            }
            Kind::Word(_) => "1;35",
            Kind::Punct(_) => "",
        };
        if code.is_empty() {
            out.push_str(text);
        } else {
            out.push_str(&paint(text, code));
        }
        at = t.to;
    }
    gap(&line[at..], &mut out);
    out
}

impl Highlighter for ShellHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if !self.color || line.is_empty() {
            return Cow::Borrowed(line);
        }
        let pending = self
            .session
            .try_borrow()
            .map(|s| s.is_pending())
            .unwrap_or(false);
        let lead = line.len() - line.trim_start().len();
        if !pending && line[lead..].starts_with('.') {
            let end = line[lead..]
                .find(char::is_whitespace)
                .map_or(line.len(), |i| lead + i);
            return Cow::Owned(format!(
                "{}{}{}",
                &line[..lead],
                paint(&line[lead..end], &format!("{BOLD};{YELLOW}")),
                &line[end..]
            ));
        }
        Cow::Owned(highlight_sparql(line))
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        if !self.color {
            return Cow::Borrowed(prompt);
        }
        match prompt.strip_suffix("> ") {
            Some(name) if prompt == PROMPT => Cow::Owned(format!(
                "{}{}",
                paint(name, &format!("{BOLD};{GREEN}")),
                paint("> ", DIM)
            )),
            _ => Cow::Owned(paint(prompt, DIM)),
        }
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if self.color {
            Cow::Owned(paint(hint, DIM))
        } else {
            Cow::Borrowed(hint)
        }
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        self.color
    }
}

/// The byte offset of UTF-16 column `col` in `line`.
fn byte_of_utf16(line: &str, col: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in line.char_indices() {
        if units >= col {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    line.len()
}

fn pairs<'a>(
    start: usize,
    typed: &str,
    words: impl IntoIterator<Item = &'a str>,
) -> (usize, Vec<Pair>) {
    let lower = typed.to_lowercase();
    (
        start,
        words
            .into_iter()
            .filter(|w| w.to_lowercase().starts_with(&lower))
            .map(|w| Pair {
                display: w.to_string(),
                replacement: w.to_string(),
            })
            .collect(),
    )
}

/// SPARQL completion at `pos` of `line`, after the lines already pending.
pub fn complete_sparql(session: &mut Session, line: &str, pos: usize) -> (usize, Vec<Pair>) {
    session.vocab();
    let session = &*session;
    let mut before = session.prefixes.prologue();
    if session.is_pending() {
        before.push_str(&session.pending);
        before.push('\n');
    }
    let line_no = before.matches('\n').count() as u32;
    let text = format!("{before}{line}");
    let cursor = Position::new(line_no, line[..pos].encode_utf16().count() as u32);
    let index = SourceIndex::default();
    let ctx = CompletionContext {
        vocab: session.vocab.as_ref(),
        index: &index,
        cypher: None,
    };
    let mut items = lang::complete(Lang::Sparql, &text, "file:///shell.rq", cursor, &ctx);
    items.sort_by(|a, b| {
        (a.sort_text.as_ref().unwrap_or(&a.label), &a.label)
            .cmp(&(b.sort_text.as_ref().unwrap_or(&b.label), &b.label))
    });
    let word_start = line[..pos]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '?' | '$'))
        .last()
        .map_or(pos, |(i, _)| i);
    let mut start = None;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let (from, replacement) = match &item.text_edit {
            Some(CompletionTextEdit::Edit(e)) if e.range.start.line == line_no => (
                byte_of_utf16(line, e.range.start.character),
                e.new_text.clone(),
            ),
            Some(_) => continue,
            None => (
                word_start,
                item.insert_text.clone().unwrap_or(item.label.clone()),
            ),
        };
        if from > pos || *start.get_or_insert(from) != from {
            continue;
        }
        let typed = line[from..pos].to_lowercase();
        if !replacement.to_lowercase().starts_with(&typed) || !seen.insert(replacement.clone()) {
            continue;
        }
        let display = match &item.detail {
            Some(d) if d.ends_with("uses") => format!("{}  ({d})", item.label),
            _ => item.label.clone(),
        };
        out.push(Pair {
            display,
            replacement,
        });
    }
    (start.unwrap_or(pos), out)
}

/// Completion of a dot-command or its argument; `None` for a file argument or a statement.
pub fn complete_command(session: &Session, line: &str, pos: usize) -> Option<(usize, Vec<Pair>)> {
    let before = &line[..pos];
    let lead = before.len() - before.trim_start().len();
    if session.is_pending() || !before[lead..].starts_with('.') {
        return None;
    }
    let Some(space) = before[lead..].find(char::is_whitespace) else {
        return Some(pairs(lead, &before[lead..], COMMANDS.iter().map(|c| c.0)));
    };
    let command = &before[lead..lead + space];
    let arg_start = before.rfind(char::is_whitespace).map_or(pos, |i| i + 1);
    let typed = &before[arg_start..];
    match command {
        ".mode" => Some(pairs(arg_start, typed, Mode::NAMES.iter().copied())),
        ".timer" => Some(pairs(arg_start, typed, ["on", "off"])),
        ".prefix" => {
            let names: Vec<String> = session
                .prefixes
                .iter()
                .map(|(p, _)| format!("{p}:"))
                .collect();
            Some(pairs(arg_start, typed, names.iter().map(String::as_str)))
        }
        ".open" | ".save" | ".load" | ".read" | ".dump" | ".datalog" => None,
        _ => Some((pos, Vec::new())),
    }
}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return Ok((pos, Vec::new()));
        };
        if let Some(done) = complete_command(&session, line, pos) {
            return Ok(done);
        }
        let lead = line.len() - line.trim_start().len();
        if !session.is_pending() && line[lead..].starts_with('.') {
            drop(session);
            return self.files.complete(line, pos, ctx);
        }
        Ok(complete_sparql(&mut session, line, pos))
    }
}

fn history_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".oxilite_history"))
}

/// The interactive loop; returns the exit status.
pub fn interact(session: &Rc<RefCell<Session>>) -> Result<i32> {
    let color = session.borrow().paint.color;
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .completion_prompt_limit(200)
        .history_ignore_dups(true)?
        .max_history_size(5000)?
        .auto_add_history(false)
        .color_mode(if color {
            ColorMode::Enabled
        } else {
            ColorMode::Disabled
        })
        .build();
    let mut editor: Editor<ShellHelper, DefaultHistory> = Editor::with_config(config)?;
    editor.set_helper(Some(ShellHelper {
        session: Rc::clone(session),
        files: FilenameCompleter::new(),
        hinter: HistoryHinter::new(),
        color,
    }));
    let history = history_path();
    if let Some(h) = &history {
        let _ = editor.load_history(h);
    }
    let save = |editor: &mut Editor<ShellHelper, DefaultHistory>| {
        if let Some(h) = &history {
            let _ = editor.save_history(h);
        }
    };
    loop {
        let prompt = if session.borrow().is_pending() {
            CONTINUATION
        } else {
            PROMPT
        };
        match editor.readline(prompt) {
            Ok(text) => {
                for line in text.split('\n') {
                    let flow = session.borrow_mut().feed_line(line);
                    if let Some(entry) = session.borrow_mut().last_entry.take() {
                        let _ = editor.add_history_entry(entry);
                    }
                    if let Flow::Exit(code) = flow {
                        save(&mut editor);
                        return Ok(code);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                let mut s = session.borrow_mut();
                if s.is_pending() {
                    s.cancel();
                } else {
                    s.note("Use .exit or Ctrl-D to quit.");
                }
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                save(&mut editor);
                return Err(e.into());
            }
        }
    }
    save(&mut editor);
    Ok(0)
}
