//! 사용자 설정 파일 경로 — `$XDG_CONFIG_HOME/inno-creed/` (없으면 `~/.config/inno-creed/`).
//!
//! 현재 쓰는 파일:
//!
//! | 파일 | 담당 | MCP 쓰기 |
//! |---|---|---|
//! | `approval_line.json` | `modules::approval_schema` (결재라인 스키마 override) | ❌ 읽기 전용 |
//! | `person_groups.json` | `modules::person_group` (사람 그룹) | ✅ 전용 도구로만 |
//!
//! ⚠️ **쓰기는 그 파일 담당 모듈만 한다.** 여기(`config`)는 경로만 정한다.
//! 사람이 손으로도 고치는 파일이므로, 쓸 때는 **읽고 → 고치고 → 통째로 다시 쓴다**
//! (`person_group::write_groups`). 부분 갱신을 흉내내면 사람이 적어둔 다른 그룹이 날아간다.

/// 설정 디렉토리. `XDG_CONFIG_HOME` 우선, 없으면 `$HOME/.config`.
/// 둘 다 없으면 `None`(설정 없이 동작한다 — 번들 기본값으로 떨어진다).
pub fn dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("inno-creed"))
}

/// 설정 디렉토리 안의 파일 경로.
pub fn file(name: &str) -> Option<std::path::PathBuf> {
    dir().map(|d| d.join(name))
}

/// 설정 디렉토리를 만든다(있으면 그대로). 쓰기 직전에 부른다 —
/// 이 디렉토리는 사용자가 미리 만들어 두지 않는 것이 보통이다.
pub fn ensure_dir() -> std::io::Result<std::path::PathBuf> {
    let d = dir().ok_or_else(|| {
        std::io::Error::other("설정 디렉토리를 정할 수 없다(HOME·XDG_CONFIG_HOME 둘 다 없음)")
    })?;
    std::fs::create_dir_all(&d)?;
    Ok(d)
}
