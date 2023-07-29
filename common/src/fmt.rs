use std::fmt;

/// Used for displaying a slice of bytes as a hexadecimal string
pub struct HexSlice<'a>(&'a [u8]);

impl<'a> From<&'a [u8]> for HexSlice<'a> {
    fn from(value: &'a [u8]) -> HexSlice<'a> {
        HexSlice(value)
    }
}

impl<'a> fmt::Display for HexSlice<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:x}")?;
        }
        Ok(())
    }
}