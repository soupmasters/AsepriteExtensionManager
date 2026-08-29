use std::path::PathBuf;

use aem_helper::package::{package_manager_directory, validate_manager_directory};
use aem_helper::state::State;

#[test]
#[ignore = "requires AEM_INSTALLED_MANAGER to name a locally installed manager directory"]
fn installed_manager_can_be_verified_and_packaged_for_recovery() {
    let extension_root = PathBuf::from(
        std::env::var_os("AEM_INSTALLED_MANAGER")
            .expect("set AEM_INSTALLED_MANAGER to the installed extension directory"),
    );
    let temporary = tempfile::tempdir().expect("tempdir");
    let state = State::new(temporary.path()).expect("state");

    let manifest = validate_manager_directory(&extension_root, aem_helper::VERSION)
        .expect("installed manager validation");
    let recovery =
        package_manager_directory(&state, &extension_root).expect("manager recovery package");

    assert_eq!(manifest.name, "aseprite-extension-manager");
    assert_eq!(recovery.version, aem_helper::VERSION);
    assert!(recovery.artifact_path.is_file());
    assert!(recovery.byte_length > 0);
}
