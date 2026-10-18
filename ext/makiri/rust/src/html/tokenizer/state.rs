//! `html/tokenizer/state.c`: data, PLAINTEXT, tags and attributes, bogus
//! comments, markup declarations, CDATA sections, processing instructions and
//! character references. Each function names the Lexbor function it ports;
//! section numbers are Lexbor's comments (the HTML Standard's 13.2.5.x).

use super::state_comment::comment_before_start;
use super::state_doctype::doctype_before;
use super::{flags, is_alnum, is_alpha, ncasecmp_first, seek4, ErrorId, Tokenizer};
use crate::compat::{self, Fix};
use crate::html::res::entities::ENTITIES;
use crate::html::res::{names as res, sbst_find};

/// A state: consume what it can of `[data, end)` and return where it stopped
/// (`lxb_html_tokenizer_state_f`).
pub type State = fn(&mut Tokenizer<'_, '_>, usize, usize) -> usize;

/// Lexbor's `if (...) { return end; }` after a helper that failed: the status
/// is already set.
macro_rules! try_or {
    ($e:expr, $end:expr) => {
        if $e.is_err() {
            return $end;
        }
    };
}
pub(crate) use try_or;

/// The `case 0x0D` every text-collecting state shares: append up to the CR,
/// turn it into LF, and skip an LF after it. At the end of the chunk the
/// decision moves to `lxb_html_tokenizer_state_cr`.
macro_rules! cr_case {
    ($t:ident, $data:ident, $end:ident, $self_state:expr) => {{
        $data += 1;
        if $data >= $end {
            try_or!($t.append_data($data - 1), $end);
            $t.state = cr;
            $t.state_return = Some($self_state);
            return $data;
        }
        try_or!($t.append_data($data), $end);
        $t.last_to_lf();
        $t.begin = $data + 1;
        if $t.at($data) != 0x0A {
            $t.begin = $data;
            $data -= 1;
        }
    }};
}
pub(crate) use cr_case;

/// `lxb_html_tokenizer_state_data_before`.
pub fn data_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    t.state = data_state;
    data
}

/// `lxb_html_tokenizer_state_data` (12.2.5.1).
fn data_state(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    data = seek4(t.input, data, end, [0x3C, 0x26, 0x0D, 0x00]);
    while data != end {
        match t.at(data) {
            0x3C => {
                try_or!(t.append_data(data), end);
                t.token.end = Some(data);
                t.state = tag_open;
                return data + 1;
            }
            0x26 => {
                try_or!(t.append_data(data + 1), end);
                t.state = char_ref;
                t.state_return = Some(data_state);
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, data_state),
            0x00 => {
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    if !t.buf.is_empty() {
                        t.token.tag_id = res::TAG__TEXT;
                        try_or!(t.append_data(data), end);
                        t.set_text();
                        try_or!(t.token_done(), end);
                    }
                    return end;
                }
                match t.token.null_count.checked_add(1) {
                    Some(n) => t.token.null_count = n,
                    None => {
                        t.status = super::Status::Overflow;
                        return end;
                    }
                }
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_plaintext_before`.
pub fn plaintext_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    t.token.tag_id = res::TAG__TEXT;
    t.state = plaintext;
    data
}

/// `lxb_html_tokenizer_state_plaintext` (12.2.5.5).
fn plaintext(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x0D => cr_case!(t, data, end, plaintext),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    t.set_text();
                    try_or!(t.token_text_done(), end);
                    return end;
                }
                t.begin = data + 1;
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_tag_open` (12.2.5.6).
fn tag_open(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if is_alpha(c) {
        t.state = tag_name;
        try_or!(t.emit_text_not_empty(), end);
        t.token_set_begin(data);
        return data;
    } else if c == 0x2F {
        t.state = end_tag_open;
        return data + 1;
    } else if c == 0x21 {
        t.state = markup_declaration_open;
        try_or!(t.emit_text_not_empty(), end);
        return data + 1;
    } else if c == 0x3F {
        t.state = processing_instruction_open;
        try_or!(t.emit_text_not_empty(), end);
        t.token_set_begin(data);
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        try_or!(t.append(b"<"), end);
        t.token.end = Some(t.last);
        try_or!(t.emit_text_not_empty(), end);
        let at = t.token.end;
        t.error_at(at, ErrorId::Eobetana);
        return end;
    }
    try_or!(t.append(b"<"), end);
    t.error(data, ErrorId::Infichoftana);
    t.state = data_state;
    data
}

/// `lxb_html_tokenizer_state_end_tag_open` (12.2.5.7).
fn end_tag_open(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if is_alpha(c) {
        t.state = tag_name;
        try_or!(t.emit_text_not_empty(), end);
        t.token_set_begin(data);
        t.token.flags |= flags::CLOSE;
        return data;
    } else if c == 0x3E {
        t.state = data_state;
        t.error(data, ErrorId::Mientana);
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        try_or!(t.append(b"</"), end);
        t.token.end = Some(t.last);
        try_or!(t.emit_text_not_empty(), end);
        let at = t.token.end;
        t.error_at(at, ErrorId::Eobetana);
        return end;
    }
    t.state = bogus_comment_before;
    t.error(data, ErrorId::Infichoftana);
    try_or!(t.emit_text_not_empty(), end);
    t.token_set_begin(data);
    data
}

/// `lxb_html_tokenizer_state_tag_name` (12.2.5.8).
fn tag_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_tag_from_buf(0), end);
                t.token.end = Some(data);
                t.state = before_attribute_name;
                return data + 1;
            }
            0x2F => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_tag_from_buf(0), end);
                t.token.end = Some(data);
                t.state = self_closing_start_tag;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                try_or!(t.append_data(data), end);
                try_or!(t.set_tag_from_buf(0), end);
                t.token.end = Some(data);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x00 => {
                if t.is_eof {
                    t.token.end = Some(t.last);
                    let at = t.token.end;
                    t.error_at(at, ErrorId::Eointa);
                    return end;
                }
                try_or!(t.append_data(data), end);
                t.begin = data + 1;
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_before_attribute_name` (12.2.5.32).
pub fn before_attribute_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {}
            0x3D => {
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                try_or!(t.append(b"="), end);
                t.error(data, ErrorId::Uneqsibeatna);
                t.state = attribute_name;
                return data + 1;
            }
            0x2F | 0x3E => {
                t.state = after_attribute_name;
                return data;
            }
            c => {
                if c == 0x00 && t.is_eof {
                    t.state = after_attribute_name;
                    return data;
                }
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                t.state = attribute_name;
                return data;
            }
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_attribute_name` (12.2.5.33).
fn attribute_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 | 0x2F | 0x3E => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_name(), end);
                t.attr_last().name_end = Some(data);
                t.state = after_attribute_name;
                return data;
            }
            0x00 => {
                if t.is_eof {
                    let last = t.last;
                    t.attr_last().name_end = Some(last);
                    t.state = after_attribute_name;
                    return data;
                }
                try_or!(t.append_data(data), end);
                t.begin = data + 1;
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
            }
            0x3D => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_name(), end);
                t.attr_last().name_end = Some(data);
                t.state = before_attribute_value;
                return data + 1;
            }
            0x22 | 0x27 | 0x3C => t.error(data, ErrorId::Unchinatna),
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_after_attribute_name` (12.2.5.34).
fn after_attribute_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {}
            0x2F => {
                t.state = self_closing_start_tag;
                return data + 1;
            }
            0x3D => {
                t.state = before_attribute_value;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                try_or!(t.token_done(), end);
                return data + 1;
            }
            c => {
                if c == 0x00 && t.is_eof {
                    let last = t.last;
                    t.error(last, ErrorId::Eointa);
                    return end;
                }
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                t.state = attribute_name;
                return data;
            }
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_before_attribute_value` (12.2.5.35).
fn before_attribute_value(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {}
            0x22 => {
                t.state = attribute_value_double_quoted;
                return data + 1;
            }
            0x27 => {
                t.state = attribute_value_single_quoted;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                t.error(data, ErrorId::Miatva);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            _ => {
                t.state = attribute_value_unquoted;
                return data;
            }
        }
        data += 1;
    }
    data
}

/// The quoted attribute value states (12.2.5.36, 12.2.5.37):
/// `lxb_html_tokenizer_state_attribute_value_double_quoted` and
/// `_single_quoted`, which differ in their quote and (an optimisation Lexbor
/// applies to the double-quoted one only) the word-at-a-time seek.
fn attribute_value_quoted<const Q: u8>(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
    this: super::State,
) -> usize {
    if t.attr_last_value_begin().is_none() && !t.is_eof {
        t.attr_set_value_begin(Some(data));
    }
    t.begin = data;
    if Q == 0x22 {
        data = seek4(t.input, data, end, [0x22, 0x26, 0x0D, 0x00]);
    }
    while data != end {
        match t.at(data) {
            c if c == Q => {
                try_or!(t.append_data(data), end);
                t.attr_last().value_end = Some(data);
                try_or!(t.set_value(), end);
                t.state = after_attribute_value_quoted;
                return data + 1;
            }
            0x26 => {
                try_or!(t.append_data(data + 1), end);
                t.state = char_ref_attr;
                t.state_return = Some(this);
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, this),
            0x00 => {
                if t.is_eof {
                    if t.attr_last_value_begin().is_some() {
                        let last = t.last;
                        t.attr_last().value_end = Some(last);
                    }
                    let last = t.last;
                    t.error(last, ErrorId::Eointa);
                    return end;
                }
                try_or!(t.append_data(data), end);
                t.begin = data + 1;
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_attribute_value_double_quoted` (12.2.5.36).
fn attribute_value_double_quoted(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    attribute_value_quoted::<0x22>(t, data, end, attribute_value_double_quoted)
}

/// `lxb_html_tokenizer_state_attribute_value_single_quoted` (12.2.5.37).
fn attribute_value_single_quoted(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    attribute_value_quoted::<0x27>(t, data, end, attribute_value_single_quoted)
}

/// `lxb_html_tokenizer_state_attribute_value_unquoted` (12.2.5.38).
fn attribute_value_unquoted(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    if t.attr_last_value_begin().is_none() && !t.is_eof {
        t.attr_set_value_begin(Some(data));
    }
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
                try_or!(t.append_data(data), end);
                t.attr_last().value_end = Some(data);
                try_or!(t.set_value(), end);
                t.state = before_attribute_name;
                return data + 1;
            }
            0x26 => {
                try_or!(t.append_data(data + 1), end);
                t.state = char_ref_attr;
                t.state_return = Some(attribute_value_unquoted);
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                try_or!(t.append_data(data), end);
                t.attr_last().value_end = Some(data);
                try_or!(t.set_value(), end);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x00 => {
                if t.is_eof {
                    if t.attr_last_value_begin().is_some() {
                        let last = t.last;
                        t.attr_last().value_end = Some(last);
                    }
                    let last = t.last;
                    t.error(last, ErrorId::Eointa);
                    return end;
                }
                try_or!(t.append_data(data), end);
                t.begin = data + 1;
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
            }
            0x22 | 0x27 | 0x3C | 0x3D | 0x60 => {
                let at = t.token.end;
                t.error_at(at, ErrorId::Unchinunatva);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_after_attribute_value_quoted` (12.2.5.39).
fn after_attribute_value_quoted(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
            t.state = before_attribute_name;
            data + 1
        }
        0x2F => {
            t.state = self_closing_start_tag;
            data + 1
        }
        0x3E => {
            t.state = data_before;
            try_or!(t.token_done(), end);
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                let last = t.last;
                t.error(last, ErrorId::Eointa);
                return end;
            }
            t.error(data, ErrorId::Miwhbeat);
            t.state = before_attribute_name;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_cr`: a CR was the chunk's last byte.
pub fn cr(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    try_or!(t.append(b"\n"), end);
    if t.at(data) == 0x0A {
        data += 1;
    }
    if let Some(s) = t.state_return {
        t.state = s;
    }
    data
}

/// `lxb_html_tokenizer_state_self_closing_start_tag` (12.2.5.40).
pub fn self_closing_start_tag(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x3E => {
            t.state = data_before;
            t.token.flags |= flags::CLOSE_SELF;
            try_or!(t.token_done(), end);
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                let at = t.token.end;
                t.error_at(at, ErrorId::Eointa);
                return end;
            }
            t.error(data, ErrorId::Unsointa);
            t.state = before_attribute_name;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_bogus_comment_before`.
pub(crate) fn bogus_comment_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    t.token.tag_id = res::TAG__EM_COMMENT;
    t.state = bogus_comment;
    data
}

/// `lxb_html_tokenizer_state_bogus_comment` (12.2.5.41).
fn bogus_comment(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x3E => {
                t.state = data_before;
                try_or!(t.append_data(data), end);
                t.token.end = Some(data);
                t.set_text();
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, bogus_comment),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    t.set_text();
                    try_or!(t.token_done(), end);
                    return end;
                }
                try_or!(t.append_replace(), end);
                t.begin = data + 1;
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_markup_declaration_open` (12.2.5.42).
fn markup_declaration_open(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    let c = t.at(data);
    if c == 0x2D {
        if end - data < 2 {
            t.state = markup_declaration_comment;
            return data + 1;
        }
        if t.at(data + 1) == 0x2D {
            t.state = comment_before_start;
            return data + 2;
        }
    } else if c == 0x44 || c == 0x64 {
        if end - data < 7 {
            t.markup = b"doctype";
            t.state = markup_declaration_doctype;
            return data;
        }
        if t.input_slice(data, 7).eq_ignore_ascii_case(b"doctype") {
            t.state = doctype_before;
            return data + 7;
        }
    } else if c == 0x5B {
        if end - data < 7 {
            t.markup = b"[CDATA[";
            t.state = markup_declaration_cdata;
            return data;
        }
        if t.input_slice(data, 7) == b"[CDATA[" {
            let ns = t.current_ns;
            if ns != res::NS_HTML && (ns != res::NS__UNDEF || t.undef_is_foreign) {
                data += 7;
                t.token_set_begin(data);
                t.state = cdata_section_before;
                return data;
            }
            t.error(data, ErrorId::Cdinhtco);
            t.state = bogus_comment_before;
            return data;
        }
    }
    if t.is_eof {
        t.token.end = Some(t.last);
        t.token.begin = t.token.end;
        /* At the end of the input nothing set the token's begin above, so the
         * text emitted before `<!` is still in the buffer; the Standard's
         * comment is empty. Lexbor writes the text into it (P9). */
        if compat::fixed(Fix::P9) {
            t.buf.clear();
        }
    }
    t.error(data, ErrorId::Inopco);
    t.state = bogus_comment_before;
    data
}

/// `lxb_html_tokenizer_state_markup_declaration_comment`: `<!-` at the end of
/// a chunk.
fn markup_declaration_comment(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if t.at(data) == 0x2D {
        t.state = comment_before_start;
        return data + 1;
    }
    try_or!(t.append(b"-"), end);
    t.error(data, ErrorId::Inopco);
    t.state = bogus_comment_before;
    data
}

/// `lxb_html_tokenizer_state_markup_declaration_doctype`: `DOCTYPE` across a
/// chunk end.
fn markup_declaration_doctype(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let avail = t.chunk_slice(data, end);
    let Some(rest) = ncasecmp_first(t.markup, avail) else {
        t.error(data, ErrorId::Inopco);
        t.state = bogus_comment_before;
        return data;
    };
    if rest.is_empty() {
        let data = data + (t.markup.len() - rest.len());
        t.state = doctype_before;
        return data;
    }
    try_or!(t.append(avail), end);
    t.markup = rest;
    end
}

/// `lxb_html_tokenizer_state_markup_declaration_cdata`: `[CDATA[` across a
/// chunk end.
fn markup_declaration_cdata(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let avail = t.chunk_slice(data, end);
    let Some(rest) = ncasecmp_first(t.markup, avail) else {
        t.error(data, ErrorId::Inopco);
        t.state = bogus_comment_before;
        return data;
    };
    if rest.is_empty() {
        let ns = t.current_ns;
        if ns != res::NS_HTML && (ns != res::NS__UNDEF || t.undef_is_foreign) {
            let data = data + (t.markup.len() - rest.len());
            t.state = cdata_section_before;
            return data;
        }
        try_or!(t.append(b"[CDATA"), end);
        t.error(data, ErrorId::Cdinhtco);
        t.state = bogus_comment_before;
        return data;
    }
    /* Keep what matched, as the DOCTYPE twin does: when the input ends
     * inside `[CDATA[`, it is the bogus comment's data. Lexbor drops it (P8). */
    if compat::fixed(Fix::P8) {
        try_or!(t.append(avail), end);
    }
    t.markup = rest;
    end
}

/// `lxb_html_tokenizer_state_cdata_section_before`.
pub fn cdata_section_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    } else {
        let last = t.last;
        t.token_set_begin(last);
    }
    t.token.tag_id = res::TAG__TEXT;
    t.state = cdata_section;
    data
}

/// `lxb_html_tokenizer_state_cdata_section` (12.2.5.69).
fn cdata_section(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x5D => {
                try_or!(t.append_data(data), end);
                t.token.end = Some(data);
                t.state = cdata_section_bracket;
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, cdata_section),
            0x00 => {
                if t.is_eof {
                    let last = t.last;
                    t.error(last, ErrorId::Eoincd);
                    if t.token.begin.is_some() {
                        try_or!(t.append_data(data), end);
                        t.token.end = Some(t.last);
                    }
                    t.set_text();
                    try_or!(t.token_text_done(), end);
                    return end;
                }
                match t.token.null_count.checked_add(1) {
                    Some(n) => t.token.null_count = n,
                    None => {
                        t.status = super::Status::Overflow;
                        return end;
                    }
                }
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_cdata_section_bracket` (12.2.5.70).
fn cdata_section_bracket(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if t.at(data) == 0x5D {
        t.state = cdata_section_end;
        return data + 1;
    }
    try_or!(t.append(b"]"), end);
    t.state = cdata_section;
    data
}

/// `lxb_html_tokenizer_state_cdata_section_end` (12.2.5.71).
fn cdata_section_end(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if c == 0x5D {
        try_or!(t.append(b"]"), end);
        return data + 1;
    } else if c == 0x3E {
        t.state = data_before;
        t.set_text();
        try_or!(t.token_text_done(), end);
        return data + 1;
    }
    try_or!(t.append(b"]]"), end);
    t.state = cdata_section;
    data
}

/// `lxb_html_tokenizer_state_processing_instruction_open` (13.2.5.72).
fn processing_instruction_open(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if is_alpha(c) || c == 0x5F {
        t.state = processing_instruction_target;
        t.token.tag_id = res::TAG__PROCESSINGINSTRUCTION;
        return data;
    }
    if t.is_eof {
        let last = t.last;
        t.error(last, ErrorId::Eoinprin);
        return end;
    }
    t.error(data, ErrorId::Infichofprinta);
    t.buf.clear();
    try_or!(t.append(b"?"), end);
    t.state = bogus_comment_before;
    data
}

/// `lxb_html_tokenizer_state_processing_instruction_target` (13.2.5.73).
fn processing_instruction_target(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        let c = t.at(data);
        match c {
            0x09 | 0x0A | 0x0D | 0x0C | 0x20 | 0x3F | 0x3E => {
                try_or!(t.append_data(data), end);
                if t.buf.eq_ignore_ascii_case(b"xml")
                    || t.buf.eq_ignore_ascii_case(b"xml-stylesheet")
                {
                    t.error(data, ErrorId::Diprinta);
                    try_or!(processing_instruction_to_comment(t), end);
                    t.state = bogus_comment_before;
                    return data;
                }
                /* The target's length, the realloc-safe target/data boundary. */
                t.token.null_count = t.buf.len();
                t.state = after_processing_instruction_target;
                return data;
            }
            0x2D | 0x5F => {}
            _ => {
                if c == 0x00 && t.is_eof {
                    let last = t.last;
                    t.error(last, ErrorId::Eoinprin);
                    return end;
                }
                if !is_alnum(c) {
                    t.error(data, ErrorId::Inprinta);
                    try_or!(t.append_data(data), end);
                    try_or!(processing_instruction_to_comment(t), end);
                    t.state = bogus_comment_before;
                    return data;
                }
            }
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_after_processing_instruction_target` (13.2.5.74).
fn after_processing_instruction_target(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0D | 0x0C | 0x20 => {}
            _ => {
                t.state = processing_instruction_data;
                return data;
            }
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_processing_instruction_data` (13.2.5.75).
fn processing_instruction_data(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data < end {
        match t.at(data) {
            0x3F => {
                try_or!(t.append_data(data), end);
                t.state = processing_instruction_questionable;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                try_or!(t.append_data(data), end);
                t.set_text();
                t.token.end = Some(data);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x0D => {
                data += 1;
                try_or!(t.append_data(data), end);
                t.last_to_lf();
                if data >= end {
                    t.state = crlf;
                    t.state_return = Some(processing_instruction_data);
                    return data;
                }
                t.begin = data;
                if t.at(data) == 0x0A {
                    data += 1;
                    t.begin = data;
                }
                continue;
            }
            0x00 if t.is_eof => {
                let last = t.last;
                t.error(last, ErrorId::Eoinprin);
                return end;
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_processing_instruction_questionable` (13.2.5.76).
fn processing_instruction_questionable(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    if t.at(data) == 0x00 && t.is_eof {
        let last = t.last;
        t.error(last, ErrorId::Eoinprin);
        return end;
    }
    if t.at(data) == 0x3E {
        t.state = data_before;
        t.set_text();
        t.token.end = Some(data);
        try_or!(t.token_done(), end);
        return data + 1;
    }
    try_or!(t.append(b"?"), end);
    t.state = processing_instruction_data;
    data
}

/// `lxb_html_tokenizer_state_processing_instruction_to_comment`: prefix the
/// buffer with `?`.
fn processing_instruction_to_comment(t: &mut Tokenizer<'_, '_>) -> Result<(), ()> {
    use crate::falloc::Reserve;
    if t.buf.falloc_reserve(1).is_err() {
        t.status = super::Status::Oom;
        return Err(());
    }
    t.buf.insert(0, b'?');
    Ok(())
}

/// `lxb_html_tokenizer_state_crlf`.
fn crlf(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if let Some(s) = t.state_return {
        t.state = s;
    }
    if t.at(data) == 0x0A {
        return data + 1;
    }
    data
}

/* ---- character references ---- */

/// `lxb_html_tokenizer_state_char_ref` (12.2.5.72).
pub fn char_ref(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    t.is_attribute = false;
    char_ref_common(t, data, end)
}

/// `lxb_html_tokenizer_state_char_ref_attr`.
fn char_ref_attr(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    t.is_attribute = true;
    char_ref_common(t, data, end)
}

/// `_lxb_html_tokenizer_state_char_ref`.
fn char_ref_common(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if is_alnum(c) {
        t.entity = 1;
        t.entity_match = None;
        t.entity_start = t.buf.len() - 1;
        t.state = char_ref_named;
        return data;
    } else if c == 0x23 {
        t.markup_pos = Some(data);
        t.entity_start = t.buf.len() - 1;
        try_or!(t.append(b"#"), end);
        t.state = char_ref_numeric;
        return data + 1;
    }
    if let Some(s) = t.state_return {
        t.state = s;
    }
    data
}

/// `lxb_html_tokenizer_state_char_ref_named` (12.2.5.73).
fn char_ref_named(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    let begin = data;
    let mut entry = t.entity;
    let mut stopped = false;
    let input: &[u8] = t.input;
    let (mut matched_end, mut matched) = (None, None);
    while data < end {
        let c = input.get(data).copied().unwrap_or(0);
        match sbst_find(&ENTITIES, entry, c) {
            None => {
                stopped = true;
                break;
            }
            Some((i, e)) => {
                if e.value != 0 {
                    matched_end = Some(data - begin);
                    matched = Some(i);
                }
                entry = e.next as usize;
            }
        }
        data += 1;
    }
    if let (Some(off), Some(i)) = (matched_end, matched) {
        t.entity_end = t.buf.len() + off;
        t.entity_match = Some(i);
    }
    if stopped {
        let consumed = t.input_slice(begin, data - begin);
        try_or!(t.append(consumed), end);
    }
    if !stopped {
        /* Wait for the next chunk. */
        t.entity = entry;
        let consumed = t.input_slice(begin, end - begin);
        try_or!(t.append(consumed), end);
        return data;
    }

    let Some(m) = t.entity_match else {
        t.state = char_ref_ambiguous_ampersand;
        return data;
    };
    if let Some(s) = t.state_return {
        t.state = s;
    }
    let matched = &ENTITIES[m];
    if t.is_attribute && matched.key != 0x3B {
        /* The next input character is the one right after the match. */
        let ch = match t.buf.get(t.entity_end + 1) {
            Some(&b) => b,
            None => t.at(data),
        };
        if ch == 0x3D || is_alnum(ch) {
            return data;
        }
    }
    if matched.key != 0x3B {
        t.error(data, ErrorId::Miseafchre);
    }
    let value = matched.value().unwrap_or_default();
    let tail_start = t.entity_end + 1;
    let tail_len = t.buf.len().saturating_sub(tail_start);
    let new_tail = t.entity_start + value.len();
    let new_len = new_tail + tail_len;
    if new_len > t.buf.len() {
        use crate::falloc::Reserve;
        if t.buf.falloc_reserve(new_len - t.buf.len()).is_err() {
            t.status = super::Status::Oom;
            return end;
        }
        t.buf.resize(new_len, 0);
    }
    t.buf
        .copy_within(tail_start..tail_start + tail_len, new_tail);
    t.buf[t.entity_start..new_tail].copy_from_slice(value);
    t.buf.truncate(new_len);
    data
}

/// `lxb_html_tokenizer_state_char_ref_ambiguous_ampersand` (12.2.5.74).
fn char_ref_ambiguous_ampersand(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    let begin = data;
    while data < end {
        let c = t.at(data);
        if !is_alnum(c) {
            let consumed = t.input_slice(begin, data - begin);
            try_or!(t.append(consumed), end);
            if c == 0x3B {
                t.error(data, ErrorId::Unnachre);
            }
            if let Some(s) = t.state_return {
                t.state = s;
            }
            return data;
        }
        data += 1;
    }
    if begin < data {
        let consumed = t.input_slice(begin, data - begin);
        try_or!(t.append(consumed), end);
    }
    data
}

/// `lxb_html_tokenizer_state_char_ref_numeric` (12.2.5.75).
fn char_ref_numeric(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    t.entity_number = 0;
    let c = t.at(data);
    if c == 0x78 || c == 0x58 {
        try_or!(t.append(&[c]), end);
        t.state = char_ref_hexademical_start;
        return data + 1;
    }
    t.state = char_ref_decimal_start;
    data
}

/// `lxb_html_tokenizer_state_char_ref_hexademical_start` (12.2.5.76).
fn char_ref_hexademical_start(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if t.at(data).is_ascii_hexdigit() {
        t.state = char_ref_hexademical;
    } else {
        t.error(data, ErrorId::Abofdiinnuchre);
        if let Some(s) = t.state_return {
            t.state = s;
        }
    }
    data
}

/// `lxb_html_tokenizer_state_char_ref_decimal_start` (12.2.5.77).
fn char_ref_decimal_start(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if t.at(data).is_ascii_digit() {
        t.state = char_ref_decimal;
    } else {
        t.error(data, ErrorId::Abofdiinnuchre);
        if let Some(s) = t.state_return {
            t.state = s;
        }
    }
    data
}

/// `lxb_html_tokenizer_state_char_ref_hexademical` (12.2.5.78).
fn char_ref_hexademical(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        let c = t.at(data);
        let Some(v) = (c as char).to_digit(16) else {
            if let Some(s) = t.state_return {
                t.state = s;
            }
            if c == b';' {
                data += 1;
            } else {
                t.error(data, ErrorId::Miseafchre);
            }
            return char_ref_numeric_end(t, data, end);
        };
        if t.entity_number <= 0x10FFFF {
            t.entity_number = (t.entity_number << 4) | v;
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_char_ref_decimal` (12.2.5.79).
fn char_ref_decimal(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        let c = t.at(data);
        let Some(v) = (c as char).to_digit(10) else {
            if let Some(s) = t.state_return {
                t.state = s;
            }
            if c == b';' {
                data += 1;
            } else {
                t.error(data, ErrorId::Miseafchre);
            }
            return char_ref_numeric_end(t, data, end);
        };
        if t.entity_number <= 0x10FFFF {
            t.entity_number = v + t.entity_number * 10;
        }
        data += 1;
    }
    data
}

/// `lexbor_str_res_replacement_character`: the code points 0x00-0x9F a
/// numeric reference stands for (windows-1252 for 0x80-0x9F, U+FFFD for 0).
const REPLACEMENT: [u32; 160] = {
    let mut t = [0u32; 160];
    let mut i = 0;
    while i < 160 {
        t[i] = i as u32;
        i += 1;
    }
    t[0] = 0xFFFD;
    let high: [u32; 32] = [
        8364, 129, 8218, 402, 8222, 8230, 8224, 8225, 710, 8240, 352, 8249, 338, 141, 381, 143,
        144, 8216, 8217, 8220, 8221, 8226, 8211, 8212, 732, 8482, 353, 8250, 339, 157, 382, 376,
    ];
    let mut j = 0;
    while j < 32 {
        t[0x80 + j] = high[j];
        j += 1;
    }
    t
};

/// `lxb_html_tokenizer_state_char_ref_numeric_end` (12.2.5.80).
fn char_ref_numeric_end(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let n = t.entity_number;
    let at = t.markup_pos;
    let replace = if n == 0x00 {
        t.error_at(at, ErrorId::Nuchre);
        true
    } else if n > 0x10FFFF {
        t.error_at(at, ErrorId::Chreouunra);
        true
    } else if (0xD800..=0xDFFF).contains(&n) {
        t.error_at(at, ErrorId::Suchre);
        true
    } else {
        if (0xFDD0..=0xFDEF).contains(&n) {
            t.error_at(at, ErrorId::Nochre);
        }
        false
    };
    let cp = if replace {
        0xFFFD
    } else {
        if n & 0xFFFE == 0xFFFE {
            t.error_at(at, ErrorId::Nochre);
        }
        if (n <= 0x1F && n != 0x09 && n != 0x0A && n != 0x0C && n != 0x20)
            || (0x7F..=0x9F).contains(&n)
        {
            t.error_at(at, ErrorId::Cochre);
        }
        if n <= 0x9F {
            REPLACEMENT[n as usize]
        } else {
            n
        }
    };
    t.buf.truncate(t.entity_start);
    let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
    let mut b = [0u8; 4];
    try_or!(t.append(ch.encode_utf8(&mut b).as_bytes()), end);
    data
}

impl<'i> Tokenizer<'i, '_> {
    /// `[data, end)` of the chunk being tokenized: the input, or at end of file
    /// Lexbor's one-byte NUL buffer.
    pub(crate) fn chunk_slice(&self, data: usize, end: usize) -> &'i [u8] {
        if data >= self.input.len() {
            return &[0];
        }
        self.input_slice(data, end - data)
    }

    /// `len` input bytes from `at`, as far as the input reaches.
    pub(crate) fn input_slice(&self, at: usize, len: usize) -> &'i [u8] {
        let input: &'i [u8] = self.input;
        let n = input.len();
        input
            .get(at.min(n)..at.saturating_add(len).min(n))
            .unwrap_or_default()
    }
}
