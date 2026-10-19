//! `html/tokenizer/state_comment.c`: comments (12.2.5.43-52).

use super::state::{cr, cr_case, data_before, try_or};
use super::{ErrorId, Tokenizer};
use crate::html::res::names as res;

/// `lxb_html_tokenizer_state_comment_before_start`.
pub fn comment_before_start(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    if !t.is_eof {
        t.token_set_begin(data);
        t.token.end = Some(data);
    }
    t.token.tag_id = res::TAG__EM_COMMENT;
    comment_start(t, data, end)
}

/// `lxb_html_tokenizer_state_comment_start` (12.2.5.43).
fn comment_start(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    match t.at(data) {
        0x2D => {
            data += 1;
            t.state = comment_start_dash;
        }
        0x3E => {
            t.state = data_before;
            t.error(data, ErrorId::Abclofemco);
            t.set_text();
            try_or!(t.token_done(), end);
            data += 1;
        }
        _ => t.state = comment,
    }
    data
}

/// The end-of-file case several comment states share: report it and emit the
/// comment so far.
fn eof_in_comment(t: &mut Tokenizer<'_, '_>, end: usize) -> usize {
    let last = t.last;
    t.error(last, ErrorId::Eoinco);
    t.set_text();
    try_or!(t.token_done(), end);
    end
}

/// `lxb_html_tokenizer_state_comment_start_dash` (12.2.5.44).
fn comment_start_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if c == 0x2D {
        t.state = comment_end;
        return data + 1;
    } else if c == 0x3E {
        t.state = data_before;
        t.error(data, ErrorId::Abclofemco);
        t.set_text();
        try_or!(t.token_done(), end);
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        return eof_in_comment(t, end);
    }
    try_or!(t.append(b"-"), end);
    t.state = comment;
    data
}

/// `lxb_html_tokenizer_state_comment` (12.2.5.45).
fn comment(t: &mut Tokenizer<'_, '_>, mut data: usize, end: usize) -> usize {
    t.begin = data;
    while data != end {
        match t.at(data) {
            0x3C => {
                data += 1;
                try_or!(t.append_data(data), end);
                t.state = comment_less_than_sign;
                return data;
            }
            0x2D => {
                t.token.end = data.checked_sub(1);
                try_or!(t.append_data(data), end);
                t.state = comment_end_dash;
                return data + 1;
            }
            0x0D => cr_case!(t, data, end, comment),
            0x00 => {
                try_or!(t.append_data(data), end);
                if t.is_eof {
                    if t.token.begin.is_some() {
                        t.token.end = Some(t.last);
                    }
                    let at = t.token.end;
                    t.error_at(at, ErrorId::Eoinco);
                    t.set_text();
                    try_or!(t.token_done(), end);
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

/// `lxb_html_tokenizer_state_comment_less_than_sign` (12.2.5.46).
fn comment_less_than_sign(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    match t.at(data) {
        0x21 => {
            try_or!(t.append(b"!"), end);
            t.state = comment_less_than_sign_bang;
            data + 1
        }
        0x3C => {
            try_or!(t.append(b"<"), end);
            data + 1
        }
        _ => {
            t.state = comment;
            data
        }
    }
}

/// `lxb_html_tokenizer_state_comment_less_than_sign_bang` (12.2.5.47).
fn comment_less_than_sign_bang(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if t.at(data) == 0x2D {
        t.state = comment_less_than_sign_bang_dash;
        return data + 1;
    }
    t.state = comment;
    data
}

/// `lxb_html_tokenizer_state_comment_less_than_sign_bang_dash` (12.2.5.48).
fn comment_less_than_sign_bang_dash(t: &mut Tokenizer<'_, '_>, data: usize, _end: usize) -> usize {
    if t.at(data) == 0x2D {
        t.state = comment_less_than_sign_bang_dash_dash;
        return data + 1;
    }
    t.state = comment_end_dash;
    data
}

/// `lxb_html_tokenizer_state_comment_less_than_sign_bang_dash_dash` (12.2.5.49).
fn comment_less_than_sign_bang_dash_dash(
    t: &mut Tokenizer<'_, '_>,
    data: usize,
    _end: usize,
) -> usize {
    let c = t.at(data);
    if c == 0x3E || (c == 0x00 && t.is_eof) {
        t.state = comment_end;
        return data;
    }
    t.error(data, ErrorId::Neco);
    t.state = comment_end;
    data
}

/// `lxb_html_tokenizer_state_comment_end_dash` (12.2.5.50).
fn comment_end_dash(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if c == 0x2D {
        t.state = comment_end;
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        return eof_in_comment(t, end);
    }
    try_or!(t.append(b"-"), end);
    t.state = comment;
    data
}

/// `lxb_html_tokenizer_state_comment_end` (12.2.5.51).
fn comment_end(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if c == 0x3E {
        t.state = data_before;
        t.set_text();
        try_or!(t.token_done(), end);
        return data + 1;
    } else if c == 0x21 {
        t.state = comment_end_bang;
        return data + 1;
    } else if c == 0x2D {
        try_or!(t.append(b"-"), end);
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        return eof_in_comment(t, end);
    }
    try_or!(t.append(b"--"), end);
    t.state = comment;
    data
}

/// `lxb_html_tokenizer_state_comment_end_bang` (12.2.5.52).
fn comment_end_bang(t: &mut Tokenizer<'_, '_>, data: usize, end: usize) -> usize {
    let c = t.at(data);
    if c == 0x2D {
        try_or!(t.append(b"--!"), end);
        t.state = comment_end_dash;
        return data + 1;
    } else if c == 0x3E {
        t.state = data_before;
        t.error(data, ErrorId::Inclco);
        t.set_text();
        try_or!(t.token_done(), end);
        return data + 1;
    } else if c == 0x00 && t.is_eof {
        return eof_in_comment(t, end);
    }
    try_or!(t.append(b"--!"), end);
    t.state = comment;
    data
}
