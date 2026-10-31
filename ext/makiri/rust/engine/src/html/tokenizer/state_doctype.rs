//! `html/tokenizer/state_doctype.c`: DOCTYPE (12.2.5.53-68).
//!
//! As in Lexbor, a DOCTYPE token carries its parts as attributes: the name
//! first, then one named `PUBLIC` or `SYSTEM` (by attribute id) whose value is
//! that identifier, then, after a public identifier, a nameless one holding
//! the system identifier.

use super::state::{cr, cr_case, data_before, try_or};
use super::{attr_flags, flags, ncasecmp_first, ErrorId, Tokenizer};
use crate::html::res::names as res;

/// `lxb_html_tokenizer_state_doctype_before`.
pub fn doctype_before(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    t.token.end = Some(if t.is_eof { t.last } else { data });
    t.token.tag_id = res::TAG__EM_DOCTYPE;
    doctype(t, data, end)
}

/// End of file in a DOCTYPE: report it, force quirks, emit the token.
fn eof_in_doctype(t: &mut Tokenizer<'_, '_>, end: usize) -> usize {
    let last = t.last;
    t.error(last, ErrorId::Eoindo);
    t.token.flags |= flags::FORCE_QUIRKS;
    try_or!(t.token_done(), end);
    end
}

/// `lxb_html_tokenizer_state_doctype` (12.2.5.53).
fn doctype(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => data += 1,
        0x3E => {}
        c => {
            if c == 0x00 && t.is_eof {
                return eof_in_doctype(t, end);
            }
            t.error(data, ErrorId::Miwhbedona);
        }
    }
    t.state = doctype_before_name;
    data
}

/// `lxb_html_tokenizer_state_doctype_before_name` (12.2.5.54).
fn doctype_before_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {}
            0x00 => {
                if t.is_eof {
                    return eof_in_doctype(t, end);
                }
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                try_or!(t.append_replace(), end);
                t.error(data, ErrorId::Unnuch);
                t.attr_last().flags |= attr_flags::NAME_NULL;
                t.state = doctype_name;
                return data + 1;
            }
            0x3E => {
                t.token.flags |= flags::FORCE_QUIRKS;
                t.state = data_before;
                t.error(data, ErrorId::Midona);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            _ => {
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                t.state = doctype_name;
                return data;
            }
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_doctype_name` (12.2.5.55).
fn doctype_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_name(), end);
                t.attr_last().name_end = Some(data);
                t.state = doctype_after_name;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                try_or!(t.append_data(data), end);
                try_or!(t.set_name(), end);
                t.attr_last().name_end = Some(data);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    let last = t.last;
                    t.attr_last().name_end = Some(last);
                    t.error(last, ErrorId::Eoindo);
                    t.token.flags |= flags::FORCE_QUIRKS;
                    try_or!(t.set_name(), end);
                    try_or!(t.token_done(), end);
                    return end;
                }
                try_or!(t.replace_null(data), end);
                t.attr_last().flags |= attr_flags::NAME_NULL;
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// Delete the last attribute (`lxb_html_token_attr_delete` on `attr_last`).
fn attr_delete_last(t: &mut Tokenizer<'_, '_>) {
    t.token.attrs.pop();
}

/// The keyword that did not follow a DOCTYPE name: bogus DOCTYPE.
fn after_name_bogus(t: &mut Tokenizer<'_, '_>, data: usize) -> usize {
    attr_delete_last(t);
    t.error(data, ErrorId::Inchseafdona);
    t.token.flags |= flags::FORCE_QUIRKS;
    t.state = doctype_bogus;
    data
}

/// `lxb_html_tokenizer_state_doctype_after_name` (12.2.5.56).
fn doctype_after_name(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {}
            0x3E => {
                t.state = data_before;
                try_or!(t.token_done(), end);
                return data + 1;
            }
            c => {
                if c == 0x00 && t.is_eof {
                    return eof_in_doctype(t, end);
                }
                try_or!(t.attr_add(), end);
                t.attr_set_name_begin(data);
                if data + 6 > end {
                    if c == 0x50 || c == 0x70 {
                        t.markup = b"PUBLIC";
                        t.state = doctype_after_name_public;
                        return data;
                    }
                    if c == 0x53 || c == 0x73 {
                        t.markup = b"SYSTEM";
                        t.state = doctype_after_name_system;
                        return data;
                    }
                } else if t.input_slice(data, 6).eq_ignore_ascii_case(b"PUBLIC") {
                    let a = t.attr_last();
                    a.name_end = Some(data + 6);
                    a.name = Some(res::ATTR_PUBLIC);
                    t.state = doctype_after_public_keyword;
                    return data + 6;
                } else if t.input_slice(data, 6).eq_ignore_ascii_case(b"SYSTEM") {
                    let a = t.attr_last();
                    a.name_end = Some(data + 6);
                    a.name = Some(res::ATTR_SYSTEM);
                    t.state = doctype_after_system_keyword;
                    return data + 6;
                }
                return after_name_bogus(t, data);
            }
        }
        data += 1;
    }
    data
}

/// `lxb_html_tokenizer_state_doctype_after_name_public` and `_system`: the
/// keyword across a chunk end.
fn after_name_keyword(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
    id: usize,
    next: super::State,
) -> usize {
    let avail = t.chunk_slice(data, end);
    let Some(rest) = ncasecmp_first(t.markup, avail) else {
        return after_name_bogus(t, data);
    };
    if rest.is_empty() {
        let pos = data + (t.markup.len() - rest.len());
        let a = t.attr_last();
        a.name_end = Some(pos);
        a.name = Some(id);
        t.state = next;
        /* Lexbor returns one past the keyword's end here. */
        return pos + 1;
    }
    t.markup = rest;
    end
}

/// `lxb_html_tokenizer_state_doctype_after_name_public`.
fn doctype_after_name_public(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    after_name_keyword(t, data, end, res::ATTR_PUBLIC, doctype_after_public_keyword)
}

/// `lxb_html_tokenizer_state_doctype_after_name_system`.
fn doctype_after_name_system(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    after_name_keyword(t, data, end, res::ATTR_SYSTEM, doctype_after_system_keyword)
}

/// What differs between the public and the system identifier's states.
struct Keyword {
    before: super::State,
    double: super::State,
    single: super::State,
    missing_ws: ErrorId,
    missing_id: ErrorId,
    missing_quote: ErrorId,
}

const PUBLIC: Keyword = Keyword {
    before: doctype_before_public_identifier,
    double: doctype_public_identifier_double_quoted,
    single: doctype_public_identifier_single_quoted,
    missing_ws: ErrorId::Miwhafdopuke,
    missing_id: ErrorId::Midopuid,
    missing_quote: ErrorId::Miqubedopuid,
};

const SYSTEM: Keyword = Keyword {
    before: doctype_before_system_identifier,
    double: doctype_system_identifier_double_quoted,
    single: doctype_system_identifier_single_quoted,
    missing_ws: ErrorId::Miwhafdosyke,
    missing_id: ErrorId::Midosyid,
    missing_quote: ErrorId::Miqubedosyid,
};

/// The keyword states (12.2.5.57, 12.2.5.63):
/// `lxb_html_tokenizer_state_doctype_after_public_keyword` and
/// `_after_system_keyword`.
fn after_keyword(t: &mut Tokenizer<'_, '_>, data: usize, end: usize, k: &Keyword) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
            t.state = k.before;
            data + 1
        }
        0x22 => {
            t.error(data, k.missing_ws);
            t.state = k.double;
            data + 1
        }
        0x27 => {
            t.error(data, k.missing_ws);
            t.state = k.single;
            data + 1
        }
        0x3E => {
            t.token.flags |= flags::FORCE_QUIRKS;
            t.state = data_before;
            t.error(data, k.missing_id);
            try_or!(t.token_done(), end);
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                t.token.flags |= flags::FORCE_QUIRKS;
                let last = t.last;
                t.error(last, ErrorId::Eoindo);
                try_or!(t.token_done(), end);
                return end;
            }
            t.token.flags |= flags::FORCE_QUIRKS;
            t.state = doctype_bogus;
            t.error(data, k.missing_quote);
            data
        }
    }
}

/// `lxb_html_tokenizer_state_doctype_after_public_keyword` (12.2.5.57).
fn doctype_after_public_keyword(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    after_keyword(t, data, end, &PUBLIC)
}

/// `lxb_html_tokenizer_state_doctype_after_system_keyword` (12.2.5.63).
fn doctype_after_system_keyword(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    after_keyword(t, data, end, &SYSTEM)
}

/// The before-identifier states (12.2.5.58, 12.2.5.64):
/// `lxb_html_tokenizer_state_doctype_before_public_identifier` and
/// `_before_system_identifier`. The public one consumes the character it
/// switches on (its cases `break` to `return data + 1`); the system one
/// returns per case, also one past it - the same, so one body serves both.
fn before_identifier(t: &mut Tokenizer<'_, '_>, data: usize, end: usize, k: &Keyword) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => data + 1,
        0x22 => {
            t.state = k.double;
            data + 1
        }
        0x27 => {
            t.state = k.single;
            data + 1
        }
        0x3E => {
            t.token.flags |= flags::FORCE_QUIRKS;
            t.state = data_before;
            t.error(data, k.missing_id);
            try_or!(t.token_done(), end);
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                let last = t.last;
                t.error(last, ErrorId::Eoindo);
                t.token.flags |= flags::FORCE_QUIRKS;
                try_or!(t.token_done(), end);
                return end;
            }
            t.error(data, k.missing_quote);
            t.token.flags |= flags::FORCE_QUIRKS;
            t.state = doctype_bogus;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_doctype_before_public_identifier` (12.2.5.58).
fn doctype_before_public_identifier(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    before_identifier(t, data, end, &PUBLIC)
}

/// `lxb_html_tokenizer_state_doctype_before_system_identifier` (12.2.5.64).
fn doctype_before_system_identifier(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    before_identifier(t, data, end, &SYSTEM)
}

/// The quoted identifier states (12.2.5.59-60, 12.2.5.65-66), one body for
/// the four Lexbor functions: quote `Q`, the state after it, the error for a
/// `>` inside it.
fn identifier_quoted<const Q: u8>(
    t: &mut Tokenizer<'_, '_>,
    mut data: usize,
    end: usize,
    this: super::State,
    after: super::State,
    abrupt: ErrorId,
) -> usize {
    if t.attr_last_value_begin().is_none() && !t.is_eof {
        t.attr_set_value_begin(Some(data));
    }
    t.begin = data;
    while data != end {
        match t.at(data) {
            c if c == Q => {
                try_or!(t.append_data(data), end);
                try_or!(t.set_value(), end);
                t.attr_last().value_end = Some(data);
                t.state = after;
                return data + 1;
            }
            0x3E => {
                t.state = data_before;
                t.token.flags |= flags::FORCE_QUIRKS;
                t.error(data, abrupt);
                try_or!(t.append_data(data), end);
                try_or!(t.set_value(), end);
                t.attr_last().value_end = Some(data);
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, this),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    let last = t.last;
                    t.attr_last().value_end = Some(last);
                    if t.attr_last_value_begin().is_none() {
                        t.attr_set_value_begin(Some(last));
                    }
                    t.error(last, ErrorId::Eoindo);
                    t.token.flags |= flags::FORCE_QUIRKS;
                    try_or!(t.set_value(), end);
                    try_or!(t.token_done(), end);
                    return end;
                }
                try_or!(t.replace_null(data), end);
                t.attr_last().flags |= attr_flags::VALUE_NULL;
            }
            _ => {}
        }
        data += 1;
    }
    try_or!(t.append_data(data), end);
    data
}

/// `lxb_html_tokenizer_state_doctype_public_identifier_double_quoted` (12.2.5.59).
fn doctype_public_identifier_double_quoted(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    identifier_quoted::<0x22>(
        t,
        data,
        end,
        doctype_public_identifier_double_quoted,
        doctype_after_public_identifier,
        ErrorId::Abdopuid,
    )
}

/// `lxb_html_tokenizer_state_doctype_public_identifier_single_quoted` (12.2.5.60).
fn doctype_public_identifier_single_quoted(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    identifier_quoted::<0x27>(
        t,
        data,
        end,
        doctype_public_identifier_single_quoted,
        doctype_after_public_identifier,
        ErrorId::Abdopuid,
    )
}

/// `lxb_html_tokenizer_state_doctype_system_identifier_double_quoted` (12.2.5.65).
fn doctype_system_identifier_double_quoted(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    identifier_quoted::<0x22>(
        t,
        data,
        end,
        doctype_system_identifier_double_quoted,
        doctype_after_system_identifier,
        ErrorId::Abdosyid,
    )
}

/// `lxb_html_tokenizer_state_doctype_system_identifier_single_quoted` (12.2.5.66).
fn doctype_system_identifier_single_quoted(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    identifier_quoted::<0x27>(
        t,
        data,
        end,
        doctype_system_identifier_single_quoted,
        doctype_after_system_identifier,
        ErrorId::Abdosyid,
    )
}

/// `lxb_html_tokenizer_state_doctype_after_public_identifier` (12.2.5.61) and
/// `_between_public_and_system_identifiers` (12.2.5.62): `between` is the
/// second, which stays put on whitespace and reports no missing whitespace.
fn after_public<const BETWEEN: bool>(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => {
            if !BETWEEN {
                t.state = doctype_between_public_and_system_identifiers;
            }
            data + 1
        }
        0x3E => {
            t.state = data_before;
            try_or!(t.token_done(), end);
            data + 1
        }
        q @ (0x22 | 0x27) => {
            if !BETWEEN {
                t.error(data, ErrorId::Miwhbedopuansyid);
            }
            try_or!(t.attr_add(), end);
            t.state = if q == 0x22 {
                doctype_system_identifier_double_quoted
            } else {
                doctype_system_identifier_single_quoted
            };
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                return eof_in_doctype(t, end);
            }
            t.error(data, ErrorId::Miqubedosyid);
            t.token.flags |= flags::FORCE_QUIRKS;
            t.state = doctype_bogus;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_doctype_after_public_identifier` (12.2.5.61).
fn doctype_after_public_identifier(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    after_public::<false>(t, data, end)
}

/// `lxb_html_tokenizer_state_doctype_between_public_and_system_identifiers` (12.2.5.62).
fn doctype_between_public_and_system_identifiers(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    end: usize,
) -> usize {
    after_public::<true>(t, data, end)
}

/// `lxb_html_tokenizer_state_doctype_after_system_identifier` (12.2.5.67).
fn doctype_after_system_identifier(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x09 | 0x0A | 0x0C | 0x0D | 0x20 => data + 1,
        0x3E => {
            t.state = data_before;
            try_or!(t.token_done(), end);
            data + 1
        }
        c => {
            if c == 0x00 && t.is_eof {
                return eof_in_doctype(t, end);
            }
            t.error(data, ErrorId::Unchafdosyid);
            t.state = doctype_bogus;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_doctype_bogus` (12.2.5.68).
fn doctype_bogus(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    while data != end {
        match t.at(data) {
            0x3E => {
                t.state = data_before;
                try_or!(t.token_done(), end);
                return data + 1;
            }
            0x00 => {
                if t.is_eof {
                    try_or!(t.token_done(), end);
                    return end;
                }
                t.error(data, ErrorId::Unnuch);
            }
            _ => {}
        }
        data += 1;
    }
    data
}
