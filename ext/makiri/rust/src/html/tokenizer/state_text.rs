//! `html/tokenizer/state_rcdata.c`, `state_rawtext.c` and `state_script.c`:
//! the states inside elements whose content is text (RCDATA 12.2.5.2/9-11,
//! RAWTEXT 12.2.5.3/12-14, script data 12.2.5.4/15-31).
//!
//! RCDATA and RAWTEXT are one body here: Lexbor's two files differ only in
//! that RCDATA decodes character references and that RAWTEXT's end tag at a
//! space or `/` records where it began. `RC` selects.

use super::state::{
    before_attribute_name, char_ref, cr, cr_case, data_before, self_closing_start_tag, try_or,
    State,
};
use super::{flags, is_alpha, ErrorId, Tokenizer};
use crate::html::res::names as res;

/* ---- RCDATA and RAWTEXT ---- */

/// `lxb_html_tokenizer_state_rcdata_before`.
pub fn rcdata_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    t.state = text::<true>;
    data
}

/// `lxb_html_tokenizer_state_rawtext_before`.
pub fn rawtext_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    t.state = text::<false>;
    data
}

/// `lxb_html_tokenizer_state_rcdata` (`RC`) / `_rawtext`.
fn text<const RC: bool>(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x3C => {
                try_or!(t.append_data(data + 1), end);
                t.token.end = Some(data);
                t.state = less_than_sign::<RC>;
                return data + 1;
            }
            0x26 if RC => {
                try_or!(t.append_data(data + 1), end);
                t.state = char_ref;
                t.state_return = Some(text::<RC>);
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, text::<RC>),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    t.token.tag_id = res::TAG__TEXT;
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

/// `lxb_html_tokenizer_state_rcdata_less_than_sign` / `_rawtext_`.
fn less_than_sign<const RC: bool>(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if t.at(data) == 0x2F {
        t.state = end_tag_open::<RC>;
        return data + 1;
    }
    t.state = text::<RC>;
    data
}

/// `lxb_html_tokenizer_state_rcdata_end_tag_open` / `_rawtext_`.
fn end_tag_open<const RC: bool>(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if is_alpha(t.at(data)) {
        t.temp = Some(data);
        t.entity_start = t.buf.len() - 1;
        t.state = end_tag_name::<RC>;
    } else {
        t.state = text::<RC>;
    }
    try_or!(t.append(b"/"), end);
    data
}

/// The appropriate-end-tag test of every `*_end_tag_name` state: the name
/// collected after `</` (`entity_start` is the `<`) interned and compared with
/// the last start tag. `Err` (status set) on allocation failure.
fn appropriate(t: &mut Tokenizer<'_, '_>) -> Result<bool, ()> {
    t.set_tag_from_buf(t.entity_start + 2)?;
    Ok(t.tmp_tag_id == t.token.tag_id)
}

/// The text so far is emitted, and the token becomes the end tag, as every
/// `*_end_tag_name` state's `done` label does. `Err` when the sink stopped.
fn emit_text_then_end_tag(
    t: &mut Tokenizer<'_, '_>,
    begin: Option<usize>,
    end_at: usize,
) -> Result<(), ()> {
    t.token.tag_id = res::TAG__TEXT;
    t.buf.truncate(t.entity_start);
    t.set_text();
    t.token_text_done()?;
    t.token.tag_id = t.tmp_tag_id;
    if let Some(b) = begin {
        t.token.begin = Some(b);
    }
    t.token.end = Some(end_at);
    t.token.flags |= flags::CLOSE;
    Ok(())
}

/// The `*_end_tag_name` states, one body for the five of them: `back` is the
/// state to return to when the tag is not the appropriate end tag, and
/// `begin_at_space` says whether the end tag at a space or `/` records where
/// it began (RAWTEXT and script do, RCDATA does not).
fn end_tag_name_common(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
    back: State,
    begin_at_space: bool,
) -> usize {
    t.begin = data;
    while data != end {
        let c = t.at(data);
        match c {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 | 0x2F | 0x3E => {
                try_or!(t.append_data(data), end);
                match appropriate(t) {
                    Err(()) => return end,
                    Ok(false) => {
                        t.state = back;
                        return data;
                    }
                    Ok(true) => {}
                }
                if c == 0x3E {
                    t.state = data_before;
                    try_or!(emit_text_then_end_tag(t, t.temp, data), end);
                    try_or!(t.token_done(), end);
                    return data + 1;
                }
                t.state = if c == 0x2F {
                    self_closing_start_tag
                } else {
                    before_attribute_name
                };
                let begin = if begin_at_space { t.temp } else { None };
                try_or!(emit_text_then_end_tag(t, begin, data), end);
                return data + 1;
            }
            _ => {
                if !is_alpha(c) {
                    try_or!(t.append_data(data), end);
                    t.state = back;
                    return data;
                }
            }
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_rcdata_end_tag_name` / `_rawtext_`.
fn end_tag_name<const RC: bool>(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    end_tag_name_common(t, data, end, text::<RC>, !RC)
}

/* ---- script data ---- */

/// `lxb_html_tokenizer_state_script_data_before`.
pub fn script_data_before(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
    }
    t.state = script_data;
    data
}

/// `lxb_html_tokenizer_state_script_data` (12.2.5.4).
fn script_data(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x3C => {
                try_or!(t.append_data(data + 1), end);
                t.token.end = Some(data);
                t.state = script_data_less_than_sign;
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, script_data),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    t.token.tag_id = res::TAG__TEXT;
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

/// `lxb_html_tokenizer_state_script_data_less_than_sign` (12.2.5.15).
fn script_data_less_than_sign(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    match t.at(data) {
        0x2F => {
            t.state = script_data_end_tag_open;
            data + 1
        }
        0x21 => {
            t.state = script_data_escape_start;
            data + 1
        }
        _ => {
            t.state = script_data;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_script_data_end_tag_open` (12.2.5.16).
fn script_data_end_tag_open(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if is_alpha(t.at(data)) {
        t.entity_start = t.buf.len() - 1;
        t.temp = Some(data);
        t.state = script_data_end_tag_name;
    } else {
        t.state = script_data;
    }
    try_or!(t.append(b"/"), end);
    data
}

/// `lxb_html_tokenizer_state_script_data_end_tag_name` (12.2.5.17).
fn script_data_end_tag_name(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    end_tag_name_common(t, data, end, script_data, true)
}

/// `lxb_html_tokenizer_state_script_data_escape_start` (12.2.5.18).
fn script_data_escape_start(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if t.at(data) == 0x2D {
        t.state = script_data_escape_start_dash;
        return data + 1;
    }
    try_or!(t.append(b"!"), end);
    t.state = script_data;
    data
}

/// `lxb_html_tokenizer_state_script_data_escape_start_dash` (12.2.5.19).
fn script_data_escape_start_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if t.at(data) == 0x2D {
        try_or!(t.append(b"!--"), end);
        t.state = script_data_escaped_dash_dash;
        return data + 1;
    }
    try_or!(t.append(b"!-"), end);
    t.state = script_data;
    data
}

/// End of file inside escaped script data, the case the escaped states share.
fn eof_in_script_comment(t: &mut Tokenizer<'_, '_>, end: usize) -> usize {
    let last = t.last;
    t.error(last, ErrorId::Eoinschtcolite);
    t.token.tag_id = res::TAG__TEXT;
    t.set_text();
    t.token.end = Some(t.last);
    try_or!(t.token_text_done(), end);
    end
}

/// `lxb_html_tokenizer_state_script_data_escaped` (12.2.5.20) and
/// `_double_escaped` (12.2.5.27), which differ only in where `<` leads and in
/// whether it marks the token's end.
fn escaped_common<const DOUBLE: bool>(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
) -> usize {
    t.begin = data;
    let this: State = if DOUBLE {
        script_data_double_escaped
    } else {
        script_data_escaped
    };
    while data != end {
        match t.at(data) {
            0x2D => {
                try_or!(t.append_data(data + 1), end);
                t.state = if DOUBLE {
                    script_data_double_escaped_dash
                } else {
                    script_data_escaped_dash
                };
                return data + 1;
            }
            0x3C => {
                try_or!(t.append_data(data + 1), end);
                if !DOUBLE {
                    t.token.end = Some(data);
                }
                t.state = if DOUBLE {
                    script_data_double_escaped_less_than_sign
                } else {
                    script_data_escaped_less_than_sign
                };
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, this),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    return eof_in_script_comment(t, end);
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

/// `lxb_html_tokenizer_state_script_data_escaped` (12.2.5.20).
fn script_data_escaped(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    escaped_common::<false>(t, data, end)
}

/// `lxb_html_tokenizer_state_script_data_escaped_dash` (12.2.5.21).
fn script_data_escaped_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x2D => {
            try_or!(t.append(b"-"), end);
            t.state = script_data_escaped_dash_dash;
            data + 1
        }
        0x3C => {
            try_or!(t.append(b"<"), end);
            t.token.end = Some(data);
            t.state = script_data_escaped_less_than_sign;
            data + 1
        }
        0x00 => {
            if t.is_eof {
                return eof_in_script_comment(t, end);
            }
            try_or!(t.append_replace(), end);
            t.error(data, ErrorId::Unnuch);
            t.state = script_data_escaped;
            data + 1
        }
        _ => {
            t.state = script_data_escaped;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_script_data_escaped_dash_dash` (12.2.5.22).
fn script_data_escaped_dash_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x2D => {
            try_or!(t.append(b"-"), end);
            data + 1
        }
        0x3C => {
            try_or!(t.append(b"<"), end);
            t.token.end = Some(data);
            t.state = script_data_escaped_less_than_sign;
            data + 1
        }
        0x3E => {
            t.state = script_data;
            data
        }
        _ => {
            t.state = script_data_escaped;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_script_data_escaped_less_than_sign` (12.2.5.23).
fn script_data_escaped_less_than_sign(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    _end: usize,
) -> usize {
    let c = t.at(data);
    if c == 0x2F {
        t.state = script_data_escaped_end_tag_open;
        return data + 1;
    }
    if is_alpha(c) {
        t.entity_start = t.buf.len();
        t.state = script_data_double_escape_start;
        return data;
    }
    t.state = script_data_escaped;
    data
}

/// `lxb_html_tokenizer_state_script_data_escaped_end_tag_open` (12.2.5.24).
fn script_data_escaped_end_tag_open(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if is_alpha(t.at(data)) {
        t.temp = Some(data);
        t.entity_start = t.buf.len() - 1;
        t.state = script_data_escaped_end_tag_name;
    } else {
        t.state = script_data_escaped;
    }
    try_or!(t.append(b"/"), end);
    data
}

/// `lxb_html_tokenizer_state_script_data_escaped_end_tag_name` (12.2.5.25).
fn script_data_escaped_end_tag_name(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    end_tag_name_common(t, data, end, script_data_escaped, true)
}

/// `lxb_html_tokenizer_state_script_data_double_escape_start` (12.2.5.26) and
/// `_double_escape_end` (12.2.5.30): after a run of letters, `script` flips
/// between escaped and double escaped.
fn double_escape_switch(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
    on_script: State,
    otherwise: State,
) -> usize {
    t.begin = data;
    while data != end {
        let c = t.at(data);
        match c {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 | 0x2F | 0x3E => {
                try_or!(t.append_data(data), end);
                let name = t.buf.get(t.entity_start..).unwrap_or_default();
                t.state = if name.len() == 6 && name.eq_ignore_ascii_case(b"script") {
                    on_script
                } else {
                    otherwise
                };
                return data;
            }
            _ => {
                if !is_alpha(c) {
                    try_or!(t.append_data(data), end);
                    t.state = otherwise;
                    return data;
                }
            }
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_script_data_double_escape_start` (12.2.5.26).
fn script_data_double_escape_start(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    double_escape_switch(
        t,
        data,
        end,
        script_data_double_escaped,
        script_data_escaped,
    )
}

/// `lxb_html_tokenizer_state_script_data_double_escaped` (12.2.5.27).
fn script_data_double_escaped(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    escaped_common::<true>(t, data, end)
}

/// `lxb_html_tokenizer_state_script_data_double_escaped_dash` (12.2.5.28).
fn script_data_double_escaped_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x2D => {
            try_or!(t.append(b"-"), end);
            t.state = script_data_double_escaped_dash_dash;
            data + 1
        }
        0x3C => {
            try_or!(t.append(b"<"), end);
            t.state = script_data_double_escaped_less_than_sign;
            data + 1
        }
        0x00 => {
            if t.is_eof {
                return eof_in_script_comment(t, end);
            }
            try_or!(t.append_replace(), end);
            t.error(data, ErrorId::Unnuch);
            t.state = script_data_double_escaped;
            data + 1
        }
        _ => {
            t.state = script_data_double_escaped;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_script_data_double_escaped_dash_dash` (12.2.5.29).
fn script_data_double_escaped_dash_dash(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    match t.at(data) {
        0x2D => {
            try_or!(t.append(b"-"), end);
            data + 1
        }
        0x3C => {
            try_or!(t.append(b"<"), end);
            t.state = script_data_double_escaped_less_than_sign;
            data + 1
        }
        0x3E => {
            try_or!(t.append(b">"), end);
            t.state = script_data;
            data + 1
        }
        0x00 => {
            if t.is_eof {
                return eof_in_script_comment(t, end);
            }
            try_or!(t.append_replace(), end);
            t.error(data, ErrorId::Unnuch);
            t.state = script_data_double_escaped;
            data + 1
        }
        _ => {
            t.state = script_data_double_escaped;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_script_data_double_escaped_less_than_sign` (12.2.5.30).
fn script_data_double_escaped_less_than_sign(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    _end: usize,
) -> usize {
    if t.at(data) == 0x2F {
        t.state = script_data_double_escaped_end_tag_open;
        return data + 1;
    }
    t.state = script_data_double_escaped;
    data
}

/// `lxb_html_tokenizer_state_script_data_double_escaped_end_tag_open`.
fn script_data_double_escaped_end_tag_open(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    if is_alpha(t.at(data)) {
        t.entity_start = t.buf.len() + 1;
        t.state = script_data_double_escape_end;
    } else {
        t.state = script_data_double_escaped;
    }
    try_or!(t.append(b"/"), end);
    data
}

/// `lxb_html_tokenizer_state_script_data_double_escape_end` (12.2.5.31).
fn script_data_double_escape_end(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    double_escape_switch(
        t,
        data,
        end,
        script_data_escaped,
        script_data_double_escaped,
    )
}

/* `cr` is reached through `cr_case!`. */
const _: State = cr;
