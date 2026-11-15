//! Default apps for file types. Phase 2 can use `xdg-mime default`.

/// Not supported yet: always fails, with -1 as the status.
pub fn set_default(_bundle_id: &str, _ext: &str) -> Result<(), i32> {
    Err(-1)
}
