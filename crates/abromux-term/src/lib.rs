//! Terminals below the UI: libghostty-vt ([`ghostty`]), the PTYs shells
//! run in ([`pty`]) and the holder that keeps them running across quits
//! ([`hold`]). POSIX only. See docs/linux-plan.md.

pub mod ghostty;
pub mod hold;
pub mod pty;
