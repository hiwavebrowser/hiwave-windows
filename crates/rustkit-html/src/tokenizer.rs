//! HTML tokenizer.
//!
//! Implements a simplified but functional HTML5 tokenizer that handles
//! the most common parsing scenarios.

use crate::entities;
use crate::{ParseError, ParseResult};
use std::collections::HashMap;

/// Token types emitted by the tokenizer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// DOCTYPE declaration
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    /// Start tag (e.g., `<div>` or `<img />`)
    StartTag {
        name: String,
        attrs: HashMap<String, String>,
        self_closing: bool,
    },
    /// End tag (e.g., `</div>`)
    EndTag { name: String },
    /// Text content
    Character(char),
    /// Comment
    Comment(String),
    /// End of file
    Eof,
}

/// Tokenization state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    TagOpen,
    TagName,
    EndTagOpen,
    EndTagName,
    SelfClosingStartTag,
    AttributeName,
    AttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeName,
    BeforeAttributeName,
    MarkupDeclarationOpen,
    CommentStart,
    Comment,
    CommentEnd,
    CommentEndDash,
    Doctype,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicId,
    DoctypePublicIdDoubleQuoted,
    DoctypePublicIdSingleQuoted,
    AfterDoctypePublicId,
    BetweenDoctypePublicAndSystemIds,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemId,
    DoctypeSystemIdDoubleQuoted,
    DoctypeSystemIdSingleQuoted,
    AfterDoctypeSystemId,
    BogusDoctype,
    BogusComment,
    RawText,
    RcData,
    ScriptData,
}

/// HTML tokenizer.
pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
    state: State,
    return_state: Option<State>,
    /// The name of the last emitted start tag (for RAWTEXT/RCDATA/ScriptData end tag matching)
    last_start_tag_name: String,
    current_tag_name: String,
    current_attrs: HashMap<String, String>,
    current_attr_name: String,
    current_attr_value: String,
    self_closing: bool,
    current_comment: String,
    doctype_name: String,
    doctype_public_id: String,
    doctype_system_id: String,
    tokens: Vec<Token>,
}

impl Tokenizer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            state: State::Data,
            return_state: None,
            last_start_tag_name: String::new(),
            current_tag_name: String::new(),
            current_attrs: HashMap::new(),
            current_attr_name: String::new(),
            current_attr_value: String::new(),
            self_closing: false,
            current_comment: String::new(),
            doctype_name: String::new(),
            doctype_public_id: String::new(),
            doctype_system_id: String::new(),
            tokens: Vec::new(),
        }
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_char(&self, offset: usize) -> Option<char> {
        self.input.get(self.pos + offset).copied()
    }

    fn consume(&mut self) -> Option<char> {
        let ch = self.current_char();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn emit(&mut self, token: Token) {
        self.tokens.push(token);
    }

    fn emit_current_comment(&mut self) {
        let comment = std::mem::take(&mut self.current_comment);
        self.emit(Token::Comment(comment));
    }

    fn emit_current_doctype(&mut self) {
        let name = std::mem::take(&mut self.doctype_name);
        let public_id = std::mem::take(&mut self.doctype_public_id);
        let system_id = std::mem::take(&mut self.doctype_system_id);
        self.emit(Token::Doctype {
            name,
            public_id,
            system_id,
        });
    }

    fn emit_current_tag(&mut self) {
        if self.current_tag_name.is_empty() {
            return;
        }
        // Save the tag name for RAWTEXT/RCDATA/ScriptData end tag matching
        self.last_start_tag_name = self.current_tag_name.clone();

        let tag = Token::StartTag {
            name: std::mem::take(&mut self.current_tag_name),
            attrs: std::mem::take(&mut self.current_attrs),
            self_closing: self.self_closing,
        };
        self.self_closing = false;
        self.emit(tag);
    }

    fn emit_current_end_tag(&mut self) {
        if !self.current_tag_name.is_empty() {
            let tag = Token::EndTag {
                name: std::mem::take(&mut self.current_tag_name),
            };
            self.emit(tag);
        }
    }

    fn emit_current_attr(&mut self) {
        if !self.current_attr_name.is_empty() {
            let name = std::mem::take(&mut self.current_attr_name);
            let value = std::mem::take(&mut self.current_attr_value);
            self.current_attrs.insert(name, value);
        }
    }

    pub fn tokenize(mut self) -> ParseResult<Vec<Token>> {
        while self.pos < self.input.len() || self.state != State::Data {
            match self.state {
                State::Data => self.state_data(),
                State::TagOpen => self.state_tag_open(),
                State::TagName => self.state_tag_name(),
                State::EndTagOpen => self.state_end_tag_open(),
                State::EndTagName => self.state_end_tag_name(),
                State::SelfClosingStartTag => self.state_self_closing_start_tag(),
                State::BeforeAttributeName => self.state_before_attribute_name(),
                State::AttributeName => self.state_attribute_name(),
                State::AfterAttributeName => self.state_after_attribute_name(),
                State::AttributeValue => self.state_attribute_value(),
                State::AttributeValueDoubleQuoted => self.state_attribute_value_double_quoted(),
                State::AttributeValueSingleQuoted => self.state_attribute_value_single_quoted(),
                State::AttributeValueUnquoted => self.state_attribute_value_unquoted(),
                State::MarkupDeclarationOpen => self.state_markup_declaration_open(),
                State::CommentStart => self.state_comment_start(),
                State::Comment => self.state_comment(),
                State::CommentEnd => self.state_comment_end(),
                State::CommentEndDash => self.state_comment_end_dash(),
                State::Doctype => self.state_doctype(),
                State::DoctypeName => self.state_doctype_name(),
                State::AfterDoctypeName => self.state_after_doctype_name(),
                State::AfterDoctypePublicKeyword => self.state_after_doctype_public_keyword(),
                State::BeforeDoctypePublicId => self.state_before_doctype_public_id(),
                State::DoctypePublicIdDoubleQuoted => self.state_doctype_public_id_double_quoted(),
                State::DoctypePublicIdSingleQuoted => self.state_doctype_public_id_single_quoted(),
                State::AfterDoctypePublicId => self.state_after_doctype_public_id(),
                State::BetweenDoctypePublicAndSystemIds => self.state_between_doctype_public_and_system_ids(),
                State::AfterDoctypeSystemKeyword => self.state_after_doctype_system_keyword(),
                State::BeforeDoctypeSystemId => self.state_before_doctype_system_id(),
                State::DoctypeSystemIdDoubleQuoted => self.state_doctype_system_id_double_quoted(),
                State::DoctypeSystemIdSingleQuoted => self.state_doctype_system_id_single_quoted(),
                State::AfterDoctypeSystemId => self.state_after_doctype_system_id(),
                State::BogusDoctype => self.state_bogus_doctype(),
                State::BogusComment => self.state_bogus_comment(),
                State::RawText => self.state_rawtext(),
                State::RcData => self.state_rcdata(),
                State::ScriptData => self.state_script_data(),
            }

            // Safety check to prevent infinite loops
            if self.pos > self.input.len() + 100 {
                return Err(ParseError::TokenizerError(
                    "Tokenizer infinite loop detected".into(),
                ));
            }
        }

        self.emit(Token::Eof);
        Ok(self.tokens)
    }

    fn state_data(&mut self) {
        match self.consume() {
            Some('<') => self.state = State::TagOpen,
            Some('&') => {
                // Decode entity in text content
                let entity = self.consume_entity();
                for ch in entity.chars() {
                    self.emit(Token::Character(ch));
                }
            }
            Some(ch) => self.emit(Token::Character(ch)),
            None => {}
        }
    }

    fn state_tag_open(&mut self) {
        match self.current_char() {
            Some('!') => {
                self.consume();
                self.state = State::MarkupDeclarationOpen;
            }
            Some('/') => {
                self.consume();
                self.state = State::EndTagOpen;
            }
            Some(ch) if ch.is_ascii_alphabetic() => {
                self.current_tag_name.clear();
                self.current_attrs.clear();
                self.state = State::TagName;
            }
            Some('?') => {
                self.consume();
                self.state = State::BogusComment;
            }
            _ => {
                self.emit(Token::Character('<'));
                self.state = State::Data;
            }
        }
    }

    fn state_tag_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.state = State::BeforeAttributeName;
            }
            Some('/') => {
                self.state = State::SelfClosingStartTag;
            }
            Some('>') => self.finish_start_tag(),
            Some(ch) => {
                self.current_tag_name.push(ch.to_ascii_lowercase());
            }
            None => {
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    /// Emit the start tag closed by the `>` just consumed, and enter the
    /// state its content is tokenized in: script data for `<script>`,
    /// RAWTEXT / RCDATA for the other text-only elements, Data otherwise.
    ///
    /// Every `>` that ends a start tag must come through here, whatever
    /// attribute state it was consumed in. When only the bare-name path did,
    /// `<script nonce="...">` fell back to Data and `a<b.length` inside it
    /// was read as a tag, truncating the script and injecting elements.
    fn finish_start_tag(&mut self) {
        self.emit_current_tag();
        self.check_special_mode();
        self.state = self.return_state.take().unwrap_or(State::Data);
    }

    fn check_special_mode(&mut self) {
        // After emitting certain tags, switch parsing mode
        // Use last_start_tag_name since current_tag_name was cleared by emit_current_tag
        match self.last_start_tag_name.as_str() {
            "script" => self.return_state = Some(State::ScriptData),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" => {
                self.return_state = Some(State::RawText)
            }
            "textarea" | "title" => self.return_state = Some(State::RcData),
            _ => self.return_state = None,
        }
    }

    fn state_end_tag_open(&mut self) {
        match self.current_char() {
            Some(ch) if ch.is_ascii_alphabetic() => {
                self.current_tag_name.clear();
                self.state = State::EndTagName;
            }
            Some('>') => {
                self.consume();
                self.state = State::Data;
            }
            None => {
                self.emit(Token::Character('<'));
                self.emit(Token::Character('/'));
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusComment;
            }
        }
    }

    fn state_end_tag_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                // Ignore whitespace after end tag name
            }
            Some('>') => {
                self.emit_current_end_tag();
                self.state = State::Data;
            }
            Some(ch) => {
                self.current_tag_name.push(ch.to_ascii_lowercase());
            }
            None => {
                self.emit_current_end_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_self_closing_start_tag(&mut self) {
        match self.consume() {
            Some('>') => {
                self.self_closing = true;
                self.emit_current_tag();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BeforeAttributeName;
            }
        }
    }

    fn state_before_attribute_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('/') | Some('>') | None => {
                self.pos -= 1;
                self.state = State::AfterAttributeName;
            }
            Some('=') => {
                // Error, but start attribute anyway
                self.current_attr_name.push('=');
                self.state = State::AttributeName;
            }
            Some(ch) => {
                self.current_attr_name.clear();
                self.current_attr_value.clear();
                self.current_attr_name.push(ch.to_ascii_lowercase());
                self.state = State::AttributeName;
            }
        }
    }

    fn state_attribute_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() || ch == '/' || ch == '>' => {
                self.pos -= 1;
                self.emit_current_attr();
                self.state = State::AfterAttributeName;
            }
            Some('=') => {
                self.state = State::AttributeValue;
            }
            Some(ch) => {
                self.current_attr_name.push(ch.to_ascii_lowercase());
            }
            None => {
                self.emit_current_attr();
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_after_attribute_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('/') => {
                self.state = State::SelfClosingStartTag;
            }
            Some('=') => {
                self.state = State::AttributeValue;
            }
            Some('>') => self.finish_start_tag(),
            Some(ch) => {
                self.current_attr_name.clear();
                self.current_attr_value.clear();
                self.current_attr_name.push(ch.to_ascii_lowercase());
                self.state = State::AttributeName;
            }
            None => {
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_attribute_value(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('"') => {
                self.state = State::AttributeValueDoubleQuoted;
            }
            Some('\'') => {
                self.state = State::AttributeValueSingleQuoted;
            }
            Some('>') => {
                self.emit_current_attr();
                self.finish_start_tag();
            }
            Some(ch) => {
                self.current_attr_value.push(ch);
                self.state = State::AttributeValueUnquoted;
            }
            None => {
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_attribute_value_double_quoted(&mut self) {
        match self.consume() {
            Some('"') => {
                self.emit_current_attr();
                self.state = State::AfterAttributeName;
            }
            Some('&') => {
                // Decode entity
                let entity = self.consume_entity();
                self.current_attr_value.push_str(&entity);
            }
            Some(ch) => {
                self.current_attr_value.push(ch);
            }
            None => {
                self.emit_current_attr();
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_attribute_value_single_quoted(&mut self) {
        match self.consume() {
            Some('\'') => {
                self.emit_current_attr();
                self.state = State::AfterAttributeName;
            }
            Some('&') => {
                let entity = self.consume_entity();
                self.current_attr_value.push_str(&entity);
            }
            Some(ch) => {
                self.current_attr_value.push(ch);
            }
            None => {
                self.emit_current_attr();
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn state_attribute_value_unquoted(&mut self) {
        match self.current_char() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.consume();
                self.emit_current_attr();
                self.state = State::BeforeAttributeName;
            }
            Some('&') => {
                self.consume();
                let entity = self.consume_entity();
                self.current_attr_value.push_str(&entity);
            }
            Some('>') => {
                self.consume();
                self.emit_current_attr();
                self.finish_start_tag();
            }
            Some(ch) => {
                self.consume();
                self.current_attr_value.push(ch);
            }
            None => {
                self.emit_current_attr();
                self.emit_current_tag();
                self.state = State::Data;
            }
        }
    }

    fn consume_entity(&mut self) -> String {
        let mut entity_str = String::from("&");

        while let Some(ch) = self.current_char() {
            if ch == ';' {
                entity_str.push(ch);
                self.consume();
                break;
            } else if ch.is_alphanumeric() || ch == '#' {
                entity_str.push(ch);
                self.consume();
            } else {
                break;
            }

            if entity_str.len() > 32 {
                break;
            }
        }

        entities::decode(&entity_str)
    }

    fn state_markup_declaration_open(&mut self) {
        // Check for comment (<!--)
        if self.current_char() == Some('-') && self.peek_char(1) == Some('-') {
            self.consume();
            self.consume();
            self.current_comment.clear();
            self.state = State::CommentStart;
            return;
        }

        // Check for DOCTYPE
        if self.matches_case_insensitive("DOCTYPE") {
            for _ in 0..7 {
                self.consume();
            }
            self.state = State::Doctype;
            return;
        }

        // Otherwise, bogus comment
        self.state = State::BogusComment;
    }

    fn matches_case_insensitive(&self, s: &str) -> bool {
        for (i, expected_ch) in s.chars().enumerate() {
            if let Some(ch) = self.peek_char(i) {
                if ch.to_ascii_uppercase() != expected_ch.to_ascii_uppercase() {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }

    fn state_comment_start(&mut self) {
        match self.current_char() {
            Some('-') => {
                self.consume();
                self.state = State::CommentEndDash;
            }
            Some('>') => {
                self.consume();
                self.emit_current_comment();
                self.state = State::Data;
            }
            _ => {
                self.state = State::Comment;
            }
        }
    }

    fn state_comment(&mut self) {
        match self.consume() {
            Some('-') => {
                self.state = State::CommentEndDash;
            }
            Some(ch) => {
                self.current_comment.push(ch);
            }
            None => {
                self.emit_current_comment();
                self.state = State::Data;
            }
        }
    }

    fn state_comment_end_dash(&mut self) {
        match self.consume() {
            Some('-') => {
                self.state = State::CommentEnd;
            }
            Some(ch) => {
                self.current_comment.push('-');
                self.current_comment.push(ch);
                self.state = State::Comment;
            }
            None => {
                self.emit_current_comment();
                self.state = State::Data;
            }
        }
    }

    fn state_comment_end(&mut self) {
        match self.consume() {
            Some('>') => {
                self.emit_current_comment();
                self.state = State::Data;
            }
            Some('-') => {
                self.current_comment.push('-');
            }
            Some(ch) => {
                self.current_comment.push('-');
                self.current_comment.push('-');
                self.current_comment.push(ch);
                self.state = State::Comment;
            }
            None => {
                self.emit_current_comment();
                self.state = State::Data;
            }
        }
    }

    fn state_doctype(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some(ch) if ch.is_ascii_alphabetic() => {
                self.doctype_name.clear();
                self.doctype_public_id.clear();
                self.doctype_system_id.clear();
                self.doctype_name.push(ch.to_ascii_lowercase());
                self.state = State::DoctypeName;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusComment;
            }
        }
    }

    fn state_doctype_name(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.state = State::AfterDoctypeName;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some(ch) => {
                self.doctype_name.push(ch.to_ascii_lowercase());
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
        }
    }

    fn state_after_doctype_name(&mut self) {
        match self.current_char() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.consume();
            }
            Some('>') => {
                self.consume();
                self.emit_current_doctype();
                self.state = State::Data;
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                // Check for PUBLIC or SYSTEM
                if self.matches_case_insensitive("PUBLIC") {
                    for _ in 0..6 { self.consume(); }
                    self.state = State::AfterDoctypePublicKeyword;
                } else if self.matches_case_insensitive("SYSTEM") {
                    for _ in 0..6 { self.consume(); }
                    self.state = State::AfterDoctypeSystemKeyword;
                } else {
                    self.state = State::BogusDoctype;
                }
            }
        }
    }

    fn state_after_doctype_public_keyword(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.state = State::BeforeDoctypePublicId;
            }
            Some('"') => {
                self.doctype_public_id.clear();
                self.state = State::DoctypePublicIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_public_id.clear();
                self.state = State::DoctypePublicIdSingleQuoted;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_before_doctype_public_id(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('"') => {
                self.doctype_public_id.clear();
                self.state = State::DoctypePublicIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_public_id.clear();
                self.state = State::DoctypePublicIdSingleQuoted;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_doctype_public_id_double_quoted(&mut self) {
        match self.consume() {
            Some('"') => {
                self.state = State::AfterDoctypePublicId;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some(ch) => {
                self.doctype_public_id.push(ch);
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
        }
    }

    fn state_doctype_public_id_single_quoted(&mut self) {
        match self.consume() {
            Some('\'') => {
                self.state = State::AfterDoctypePublicId;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some(ch) => {
                self.doctype_public_id.push(ch);
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
        }
    }

    fn state_after_doctype_public_id(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.state = State::BetweenDoctypePublicAndSystemIds;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some('"') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdSingleQuoted;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_between_doctype_public_and_system_ids(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some('"') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdSingleQuoted;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_after_doctype_system_keyword(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {
                self.state = State::BeforeDoctypeSystemId;
            }
            Some('"') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdSingleQuoted;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_before_doctype_system_id(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('"') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdDoubleQuoted;
            }
            Some('\'') => {
                self.doctype_system_id.clear();
                self.state = State::DoctypeSystemIdSingleQuoted;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_doctype_system_id_double_quoted(&mut self) {
        match self.consume() {
            Some('"') => {
                self.state = State::AfterDoctypeSystemId;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some(ch) => {
                self.doctype_system_id.push(ch);
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
        }
    }

    fn state_doctype_system_id_single_quoted(&mut self) {
        match self.consume() {
            Some('\'') => {
                self.state = State::AfterDoctypeSystemId;
            }
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            Some(ch) => {
                self.doctype_system_id.push(ch);
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
        }
    }

    fn state_after_doctype_system_id(&mut self) {
        match self.consume() {
            Some(ch) if ch.is_ascii_whitespace() => {}
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                self.state = State::BogusDoctype;
            }
        }
    }

    fn state_bogus_doctype(&mut self) {
        // Consume until '>' then emit the doctype
        match self.consume() {
            Some('>') => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            None => {
                self.emit_current_doctype();
                self.state = State::Data;
            }
            _ => {
                // Stay in bogus doctype state
            }
        }
    }

    fn state_bogus_comment(&mut self) {
        loop {
            match self.consume() {
                Some('>') => {
                    self.emit_current_comment();
                    self.state = State::Data;
                    break;
                }
                Some(ch) => {
                    self.current_comment.push(ch);
                }
                None => {
                    self.emit_current_comment();
                    self.state = State::Data;
                    break;
                }
            }
        }
    }

    fn state_rawtext(&mut self) {
        // RAWTEXT mode (for style, xmp, iframe, noembed, noframes)
        match self.consume() {
            Some('<') if self.current_char() == Some('/') => {
                self.consume(); // '/'
                // Try to match end tag
                if self.matches_end_tag() {
                    // Consume the tag name
                    self.current_tag_name = self.last_start_tag_name.clone();
                    for _ in 0..self.last_start_tag_name.len() {
                        self.consume();
                    }
                    // Skip any whitespace
                    while let Some(ch) = self.current_char() {
                        if ch.is_ascii_whitespace() {
                            self.consume();
                        } else {
                            break;
                        }
                    }
                    // Consume the '>' if present
                    if self.current_char() == Some('>') {
                        self.consume();
                    }
                    self.emit_current_end_tag();
                    self.state = State::Data;
                } else {
                    self.emit(Token::Character('<'));
                    self.emit(Token::Character('/'));
                }
            }
            Some(ch) => {
                self.emit(Token::Character(ch));
            }
            None => {
                self.state = State::Data;
            }
        }
    }

    fn state_rcdata(&mut self) {
        // RCDATA mode (for textarea, title)
        match self.consume() {
            Some('&') => {
                let entity = self.consume_entity();
                for ch in entity.chars() {
                    self.emit(Token::Character(ch));
                }
            }
            Some('<') if self.current_char() == Some('/') => {
                self.consume(); // '/'
                if self.matches_end_tag() {
                    // Consume the tag name
                    self.current_tag_name = self.last_start_tag_name.clone();
                    for _ in 0..self.last_start_tag_name.len() {
                        self.consume();
                    }
                    // Skip any whitespace
                    while let Some(ch) = self.current_char() {
                        if ch.is_ascii_whitespace() {
                            self.consume();
                        } else {
                            break;
                        }
                    }
                    // Consume the '>' if present
                    if self.current_char() == Some('>') {
                        self.consume();
                    }
                    self.emit_current_end_tag();
                    self.state = State::Data;
                } else {
                    self.emit(Token::Character('<'));
                    self.emit(Token::Character('/'));
                }
            }
            Some(ch) => {
                self.emit(Token::Character(ch));
            }
            None => {
                self.state = State::Data;
            }
        }
    }

    fn state_script_data(&mut self) {
        // Script data mode
        match self.consume() {
            Some('<') if self.current_char() == Some('/') => {
                self.consume(); // '/'
                if self.matches_end_tag() {
                    // Consume the tag name
                    self.current_tag_name = self.last_start_tag_name.clone();
                    for _ in 0..self.last_start_tag_name.len() {
                        self.consume();
                    }
                    // Skip any whitespace
                    while let Some(ch) = self.current_char() {
                        if ch.is_ascii_whitespace() {
                            self.consume();
                        } else {
                            break;
                        }
                    }
                    // Consume the '>' if present
                    if self.current_char() == Some('>') {
                        self.consume();
                    }
                    self.emit_current_end_tag();
                    self.state = State::Data;
                } else {
                    self.emit(Token::Character('<'));
                    self.emit(Token::Character('/'));
                }
            }
            Some(ch) => {
                self.emit(Token::Character(ch));
            }
            None => {
                self.state = State::Data;
            }
        }
    }

    fn matches_end_tag(&self) -> bool {
        // Check if the upcoming characters match the last start tag name
        // followed by a valid end tag terminator (whitespace, /, or >)
        if self.last_start_tag_name.is_empty() {
            return false;
        }

        let tag_name = &self.last_start_tag_name;
        for (i, expected_ch) in tag_name.chars().enumerate() {
            match self.peek_char(i) {
                Some(ch) if ch.to_ascii_lowercase() == expected_ch => continue,
                _ => return false,
            }
        }

        // After the tag name, must be whitespace, /, or >
        match self.peek_char(tag_name.len()) {
            Some(ch) if ch.is_ascii_whitespace() || ch == '/' || ch == '>' => true,
            _ => false,
        }
    }
}

/// Tokenize HTML input.
pub fn tokenize(input: &str) -> ParseResult<Vec<Token>> {
    let tokenizer = Tokenizer::new(input);
    tokenizer.tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_tag() {
        let tokens = tokenize("<div></div>").unwrap();
        assert!(matches!(tokens[0], Token::StartTag { ref name, .. } if name == "div"));
        assert!(matches!(tokens[1], Token::EndTag { ref name } if name == "div"));
    }

    #[test]
    fn test_self_closing_tag() {
        let tokens = tokenize("<br/>").unwrap();
        assert!(matches!(
            tokens[0],
            Token::StartTag { ref name, self_closing: true, .. } if name == "br"
        ));
    }

    #[test]
    fn test_attributes() {
        let tokens = tokenize("<div id=\"test\" class=\"foo\"></div>").unwrap();
        if let Token::StartTag { name, attrs, .. } = &tokens[0] {
            assert_eq!(name, "div");
            assert_eq!(attrs.get("id"), Some(&"test".to_string()));
            assert_eq!(attrs.get("class"), Some(&"foo".to_string()));
        } else {
            panic!("Expected StartTag");
        }
    }

    #[test]
    fn test_text_content() {
        let tokens = tokenize("<p>Hello World</p>").unwrap();
        assert!(matches!(tokens[0], Token::StartTag { ref name, .. } if name == "p"));
        // Text tokens are individual characters
        assert!(matches!(tokens[1], Token::Character('H')));
    }

    #[test]
    fn test_comment() {
        let tokens = tokenize("<!-- comment -->").unwrap();
        assert!(matches!(tokens[0], Token::Comment(ref s) if s == " comment "));
    }

    #[test]
    fn test_doctype() {
        let tokens = tokenize("<!DOCTYPE html>").unwrap();
        assert!(matches!(
            tokens[0],
            Token::Doctype { ref name, .. } if name == "html"
        ));
    }

    #[test]
    fn test_entity_in_attribute() {
        let tokens = tokenize("<a href=\"?foo=1&amp;bar=2\"></a>").unwrap();
        if let Token::StartTag { attrs, .. } = &tokens[0] {
            assert_eq!(attrs.get("href"), Some(&"?foo=1&bar=2".to_string()));
        } else {
            panic!("Expected StartTag");
        }
    }

    #[test]
    fn test_malformed_tag() {
        let tokens = tokenize("<div<p>").unwrap();
        // Should recover gracefully
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_empty_input() {
        let tokens = tokenize("").unwrap();
        assert!(matches!(tokens[0], Token::Eof));
    }

    #[test]
    fn test_unquoted_attribute_value() {
        let tokens = tokenize("<div class=test>").unwrap();
        if let Token::StartTag { attrs, .. } = &tokens[0] {
            assert_eq!(attrs.get("class"), Some(&"test".to_string()));
        } else {
            panic!("Expected StartTag");
        }
    }

    #[test]
    fn test_nested_tags() {
        let tokens = tokenize("<div><span><b>text</b></span></div>").unwrap();
        let tag_names: Vec<String> = tokens
            .iter()
            .filter_map(|t| match t {
                Token::StartTag { name, .. } => Some(name.clone()),
                Token::EndTag { name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tag_names, vec!["div", "span", "b", "b", "span", "div"]);
    }

    #[test]
    fn test_multiple_attributes() {
        let tokens = tokenize("<input type=\"text\" name=\"foo\" value=\"bar\" disabled>").unwrap();
        if let Token::StartTag { attrs, .. } = &tokens[0] {
            assert_eq!(attrs.len(), 4);
            assert_eq!(attrs.get("type"), Some(&"text".to_string()));
            assert_eq!(attrs.get("name"), Some(&"foo".to_string()));
            assert_eq!(attrs.get("value"), Some(&"bar".to_string()));
            assert_eq!(attrs.get("disabled"), Some(&"".to_string()));
        } else {
            panic!("Expected StartTag");
        }
    }

    /// The text between a start tag and `</name>`, and the tag names seen.
    fn content_and_tags(html: &str) -> (String, Vec<String>) {
        let tokens = tokenize(html).unwrap();
        let text = tokens
            .iter()
            .filter_map(|t| match t {
                Token::Character(c) => Some(*c),
                _ => None,
            })
            .collect();
        let tags = tokens
            .iter()
            .filter_map(|t| match t {
                Token::StartTag { name, .. } => Some(name.clone()),
                Token::EndTag { name } => Some(format!("/{name}")),
                _ => None,
            })
            .collect();
        (text, tags)
    }

    #[test]
    fn a_script_with_attributes_is_script_data() {
        // google.com's first inline script: `a<w.length` read as a tag cut
        // it to 73 of 303 bytes.
        let js = "for(var a=0;a<w.length;a+=2)x='</div><b>'+a";
        for open in [
            r#"<script nonce="cx5R">"#,
            "<script nonce='cx5R'>",
            "<script nonce=cx5R>",
            "<script async>",
            "<script async >",
            "<script type=\"text/javascript\" defer>",
        ] {
            let (text, tags) = content_and_tags(&format!("{open}{js}</script><p>"));
            assert_eq!(text, js, "{open}");
            assert_eq!(tags, vec!["script", "/script", "p"], "{open}");
        }
    }

    #[test]
    fn rawtext_and_rcdata_with_attributes() {
        let (text, tags) = content_and_tags("<style media=\"all\">a<b{}</style>");
        assert_eq!(text, "a<b{}");
        assert_eq!(tags, vec!["style", "/style"]);

        let (text, tags) = content_and_tags("<textarea name=q><b>&amp;</textarea>");
        assert_eq!(text, "<b>&", "RCDATA decodes entities but makes no tags");
        assert_eq!(tags, vec!["textarea", "/textarea"]);
    }
}

