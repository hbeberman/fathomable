use std::ffi::OsString;
use std::path::Path;

use fathomable_core::XdgDirs;

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    move |name| {
        vars.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| OsString::from(value))
    }
}

#[test]
fn explicit_variables_win() {
    let dirs = XdgDirs::resolve(env(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "/cfg"),
        ("XDG_STATE_HOME", "/st"),
        ("XDG_RUNTIME_DIR", "/run/u"),
    ]));
    assert_eq!(dirs.config_dir(), Path::new("/cfg/fathomable"));
    assert_eq!(dirs.state_dir(), Path::new("/st/fathomable"));
    assert_eq!(dirs.log_dir(), Path::new("/st/fathomable/log"));
    assert_eq!(
        dirs.runtime_dir().as_deref(),
        Some(Path::new("/run/u/fathomable"))
    );
}

#[test]
fn falls_back_to_home_and_no_runtime_dir() {
    let dirs = XdgDirs::resolve(env(&[("HOME", "/home/u"), ("XDG_STATE_HOME", "")]));
    assert_eq!(dirs.config_dir(), Path::new("/home/u/.config/fathomable"));
    assert_eq!(
        dirs.state_dir(),
        Path::new("/home/u/.local/state/fathomable")
    );
    assert_eq!(dirs.runtime_dir(), None);
}
