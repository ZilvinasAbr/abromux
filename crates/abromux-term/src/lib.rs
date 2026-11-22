//! Terminals below the UI: libghostty-vt ([`ghostty`]), the PTYs shells
//! run in ([`pty`]), the server that keeps them running across quits
//! ([`hold`]), and terminals on another machine's server ([`remote`]). POSIX only. See docs/linux-plan.md.

pub mod ghostty;
pub mod hold;
pub mod pty;
pub mod remote;
