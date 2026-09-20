pub const GENESIS_SUITE: u8 = 0x01;

pub fn validate_suite(suite_id: u8) -> crate::Result<()> {
    match suite_id {
        GENESIS_SUITE => Ok(()),
        other => Err(crate::Error::UnknownSuite(other)),
    }
}
