//! Minimal Keccak-256 implementation for Ethereum selectors and comptime folds.

const ROUNDS: usize = 24;
const RATE: usize = 136;

const ROUND_CONSTANTS: [u64; ROUNDS] = [
    0x0000_0000_0000_0001,
    0x0000_0000_0000_8082,
    0x8000_0000_0000_808a,
    0x8000_0000_8000_8000,
    0x0000_0000_0000_808b,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8009,
    0x0000_0000_0000_008a,
    0x0000_0000_0000_0088,
    0x0000_0000_8000_8009,
    0x0000_0000_8000_000a,
    0x0000_0000_8000_808b,
    0x8000_0000_0000_008b,
    0x8000_0000_0000_8089,
    0x8000_0000_0000_8003,
    0x8000_0000_0000_8002,
    0x8000_0000_0000_0080,
    0x0000_0000_0000_800a,
    0x8000_0000_8000_000a,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8080,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8008,
];

const ROTATION_OFFSETS: [u32; 25] = [
    0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14,
];

/// Computes Ethereum Keccak-256, not FIPS SHA3-256.
pub fn keccak256(input: &[u8]) -> [u8; 32] {
    let mut state = [0u64; 25];
    let mut chunks = input.chunks_exact(RATE);
    for block in chunks.by_ref() {
        absorb_block(&mut state, block);
        keccak_f1600(&mut state);
    }

    let rem = chunks.remainder();
    let mut block = [0u8; RATE];
    block[..rem.len()].copy_from_slice(rem);
    block[rem.len()] ^= 0x01;
    block[RATE - 1] ^= 0x80;
    absorb_block(&mut state, &block);
    keccak_f1600(&mut state);

    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = ((state[index / 8] >> (8 * (index % 8))) & 0xff) as u8;
    }
    out
}

fn absorb_block(state: &mut [u64; 25], block: &[u8]) {
    for (lane, bytes) in block.chunks_exact(8).enumerate() {
        state[lane] ^= u64::from_le_bytes(bytes.try_into().expect("lane is eight bytes"));
    }
}

fn keccak_f1600(state: &mut [u64; 25]) {
    for &rc in &ROUND_CONSTANTS {
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }

        let mut d = [0u64; 5];
        for x in 0..5 {
            d[x] = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
        }
        for y in 0..5 {
            for x in 0..5 {
                state[x + 5 * y] ^= d[x];
            }
        }

        let mut b = [0u64; 25];
        for y in 0..5 {
            for x in 0..5 {
                let idx = x + 5 * y;
                let dst = y + 5 * ((2 * x + 3 * y) % 5);
                b[dst] = state[idx].rotate_left(ROTATION_OFFSETS[idx]);
            }
        }

        for y in 0..5 {
            for x in 0..5 {
                state[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[((x + 1) % 5) + 5 * y]) & b[((x + 2) % 5) + 5 * y]);
            }
        }

        state[0] ^= rc;
    }
}

#[cfg(test)]
mod tests {
    use super::keccak256;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn keccak256_known_vectors() {
        assert_eq!(
            hex(&keccak256(b"")),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert_eq!(
            hex(&keccak256(b"abc")),
            "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
        );
        assert_eq!(
            &hex(&keccak256(b"transfer(address,uint256)"))[..8],
            "a9059cbb"
        );
    }
}
