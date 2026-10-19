//! The insertion modes, one module per Lexbor file
//! (`html/tree/insertion_mode/*.c`).

pub mod after_after_body;
pub mod after_after_frameset;
pub mod after_body;
pub mod after_frameset;
pub mod after_head;
pub mod before_head;
pub mod before_html;
pub mod foreign_content;
pub mod in_body;
pub mod in_caption;
pub mod in_cell;
pub mod in_column_group;
pub mod in_frameset;
pub mod in_head;
pub mod in_head_noscript;
pub mod in_row;
pub mod in_table;
pub mod in_table_body;
pub mod in_table_text;
pub mod in_template;
pub mod initial;
pub mod text;
