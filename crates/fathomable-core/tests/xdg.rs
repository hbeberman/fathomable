//! Behaviour of XDG directory resolution from the environment (ADR 0001).

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
    ]));
    assert_eq!(dirs.config_dir(), Path::new("/cfg/fathomable"));
    assert_eq!(dirs.state_dir(), Path::new("/st/fathomable"));
    assert_eq!(dirs.log_dir(), Path::new("/st/fathomable/log"));
}

#[test]
fn falls_back_to_home() {
    let dirs = XdgDirs::resolve(env(&[("HOME", "/home/u"), ("XDG_STATE_HOME", "")]));
    assert_eq!(dirs.config_dir(), Path::new("/home/u/.config/fathomable"));
    assert_eq!(
        dirs.state_dir(),
        Path::new("/home/u/.local/state/fathomable")
    );
}
