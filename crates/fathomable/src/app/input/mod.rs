//! Input: the binding table, the key dispatcher, and the mouse.
//!
//! A key press becomes a [`bindings::Chord`], is looked up with the keys
//! typed before it on the focused surface, and fires an
//! [`bindings::Action`] through [`App::act`](super::App::act). The mouse
//! is hand-written: it goes to the pane under the pointer, not the
//! focused one.

pub(crate) mod bindings;
pub(crate) mod keys;
pub(crate) mod mouse;
