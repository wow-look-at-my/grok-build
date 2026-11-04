pub struct GitContext {
    pub is_git_repo: bool,
}

pub fn collect_git_context(cwd: &str) -> GitContext {
    GitContext {
        is_git_repo: xai_gix_status::discover::discover(std::path::Path::new(cwd)).is_some(),
    }
}
