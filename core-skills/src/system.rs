pub(crate) use ody_skills::install_system_skills;
pub(crate) use ody_skills::system_cache_root_dir;

use ody_utils_absolute_path::AbsolutePathBuf;

pub(crate) fn uninstall_system_skills(ody_home: &AbsolutePathBuf) {
    let _ = std::fs::remove_dir_all(system_cache_root_dir(ody_home));
}
