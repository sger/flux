//! The tests in `tests/flume/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod flume_build_plan_tests;
mod flume_edit_tests;
mod flume_index_tests;
mod flume_lock_tests;
mod flume_manifest_tests;
mod flume_plan_tests;
mod flume_resolve_tests;
mod flume_toml_tests;
mod flume_version_tests;
mod package_build_tests;
mod package_cli_tests;
mod package_git_tests;
mod package_registry_tests;
mod package_store_publish_workspace_tests;
mod package_tree_tests;
mod project_root_tests;
