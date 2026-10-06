use anyhow::{Context, Result, bail};
use std::io::Read;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
// 512 MB: no single session should decompress larger than this
const MAX_DECOMPRESS_SIZE: u64 = 512 * 1024 * 1024;

pub fn compress(data: &[u8], level: i32) -> Result<Vec<u8>> {
    zstd::encode_all(data, level).context("zstd compression failed")
}

pub fn maybe_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() >= 4 && data[..4] == ZSTD_MAGIC {
        let mut decoder = zstd::Decoder::new(data)?;
        let mut output = Vec::new();
        decoder
            .by_ref()
            .take(MAX_DECOMPRESS_SIZE)
            .read_to_end(&mut output)
            .context("zstd decompression failed")?;
        let mut probe = [0u8; 1];
        if decoder.read(&mut probe).unwrap_or(0) > 0 {
            bail!(
                "decompressed data exceeds {} MB limit",
                MAX_DECOMPRESS_SIZE / (1024 * 1024)
            );
        }
        Ok(output)
    } else {
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let input = b"hello world, this is some test data for compression";
        let compressed = compress(input, 3).unwrap();
        assert_ne!(compressed, input);
        assert_eq!(compressed[..4], ZSTD_MAGIC);
        let decompressed = maybe_decompress(&compressed).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn passthrough_non_zstd() {
        let input = b"this is plain text, not zstd";
        let output = maybe_decompress(input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn empty_input() {
        let compressed = compress(b"", 3).unwrap();
        let decompressed = maybe_decompress(&compressed).unwrap();
        assert_eq!(decompressed, b"");
    }

    #[test]
    fn passthrough_short_input() {
        let input = b"abc";
        let output = maybe_decompress(input).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn passthrough_empty() {
        let output = maybe_decompress(b"").unwrap();
        assert_eq!(output, b"");
    }

    #[test]
    fn level_1_and_19_both_roundtrip() {
        let input = b"repeated repeated repeated repeated repeated";
        for level in [1, 19] {
            let compressed = compress(input, level).unwrap();
            let decompressed = maybe_decompress(&compressed).unwrap();
            assert_eq!(decompressed, input, "failed at level {level}");
        }
    }
}
