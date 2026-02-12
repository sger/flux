pub mod complexity;
pub mod constant_fold;
pub mod desugar;
pub mod fold;
pub mod free_vars;
pub mod rename;
pub mod tail_position;
pub mod visit;

pub use complexity::analyze_complexity;
pub use constant_fold::constant_fold;
pub use desugar::desugar;
pub use fold::{
    Folder, fold_block, fold_expr, fold_match_arm, fold_pat, fold_program, fold_stmt,
    fold_string_part,
};
pub use free_vars::{collect_free_vars, collect_free_vars_in_program};
pub use rename::{rename, rename_expr};
pub use tail_position::{TailCall, find_tail_calls};
pub use visit::{
    Visitor, walk_block, walk_expr, walk_match_arm, walk_pat, walk_program, walk_stmt,
    walk_string_part,
};
