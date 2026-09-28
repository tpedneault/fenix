//! The CCSDS Reed-Solomon (255,223) code of 131.0-B: symbols over GF(2^8)
//! with field polynomial x^8+x^7+x^2+x+1, the generator's roots at
//! alpha^(11j) for j = 112..143, and symbols on the wire in Berlekamp's
//! dual basis. Corrects up to 16 symbol errors per code word; code words
//! are interleaved to depth I (1..8) in a code block, and a shortened code
//! block has "virtual fill" -- leading zero symbols never sent.
//!
//! The decoder is the classic Berlekamp-Massey, Chien search and Forney
//! sequence (as in Phil Karn's widely used implementation), on
//! conventional-basis symbols; the dual-basis conversion happens at the
//! edges.

// The algorithm is index arithmetic on polynomial coefficients; the
// loops read as the mathematics does.
#![allow(clippy::needless_range_loop)]

pub const STANDARD: &str = "CCSDS 131.0-B";
const NN: usize = 255;
const NROOTS: usize = 32;
const FCR: usize = 112;
const PRIM: usize = 11;
/// The inverse of PRIM modulo 255: 11 * 116 = 1276 = 5 * 255 + 1.
const IPRIM: usize = 116;
const A0: usize = NN;
const GFPOLY: usize = 0x187;

struct Tables {
    alpha_to: [u8; 256],
    index_of: [usize; 256],
    genpoly: [usize; NROOTS + 1],
    /// Conventional to dual basis, and back.
    tal: [u8; 256],
    tal1: [u8; 256],
}

fn modnn(mut x: usize) -> usize {
    while x >= NN {
        x -= NN;
        x = (x >> 8) + (x & NN);
    }
    x
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut alpha_to = [0u8; 256];
        let mut index_of = [0usize; 256];
        index_of[0] = A0;
        alpha_to[A0] = 0;
        let mut sr: usize = 1;
        for i in 0..NN {
            index_of[sr] = i;
            alpha_to[i] = sr as u8;
            sr <<= 1;
            if sr & 0x100 != 0 {
                sr ^= GFPOLY;
            }
            sr &= NN;
        }
        // The generator polynomial, in index form.
        let mut g = [0usize; NROOTS + 1];
        g[0] = 1;
        let mut root = FCR * PRIM;
        for i in 0..NROOTS {
            g[i + 1] = 1;
            for j in (1..=i).rev() {
                g[j] = if g[j] != 0 { g[j - 1] ^ alpha_to[modnn(index_of[g[j]] + root)] as usize } else { g[j - 1] };
            }
            g[0] = alpha_to[modnn(index_of[g[0]] + root)] as usize;
            root += PRIM;
        }
        let mut genpoly = [0usize; NROOTS + 1];
        for i in 0..=NROOTS {
            genpoly[i] = index_of[g[i]];
        }
        // Berlekamp's dual basis (the "Tal" matrix of 131.0-B's annex).
        let rows: [u8; 8] = [0x8d, 0xef, 0xec, 0x86, 0xfa, 0x99, 0xaf, 0x7b];
        let mut tal = [0u8; 256];
        let mut tal1 = [0u8; 256];
        for i in 0..256usize {
            let mut v = 0u8;
            for j in 0..8 {
                for k in 0..8 {
                    if i & (1 << k) != 0 {
                        v ^= rows[7 - k] & (1 << j);
                    }
                }
            }
            tal[i] = v;
            tal1[v as usize] = i as u8;
        }
        Tables { alpha_to, index_of, genpoly, tal, tal1 }
    })
}

/// The 32 check symbols for 223 - `pad` data symbols (dual basis in and
/// out).
pub fn encode(data: &[u8], pad: usize) -> [u8; NROOTS] {
    let t = tables();
    let mut parity = [0u8; NROOTS];
    for &d in data.iter().take(NN - NROOTS - pad) {
        let feedback = t.index_of[(t.tal1[d as usize] ^ parity[0]) as usize];
        if feedback != A0 {
            for j in 1..NROOTS {
                parity[j] ^= t.alpha_to[modnn(feedback + t.genpoly[NROOTS - j])];
            }
        }
        parity.copy_within(1.., 0);
        parity[NROOTS - 1] = if feedback != A0 { t.alpha_to[modnn(feedback + t.genpoly[0])] } else { 0 };
    }
    let mut out = [0u8; NROOTS];
    for (o, p) in out.iter_mut().zip(parity) {
        *o = t.tal[p as usize];
    }
    out
}

/// How a code word came out of decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    /// Corrected, at these symbol positions (0 = first symbol sent).
    Corrected(Vec<usize>),
    /// Too many errors to correct: left as it was.
    Uncorrectable,
}

/// Decodes one code word in place: `word` is the 255 - `pad` symbols sent
/// (dual basis), data then check symbols.
pub fn decode(word: &mut [u8], pad: usize) -> Outcome {
    let t = tables();
    let n = NN - pad;
    if word.len() != n {
        return Outcome::Uncorrectable;
    }
    let mut data: Vec<u8> = word.iter().map(|&b| t.tal1[b as usize]).collect();
    // Syndromes.
    let mut s = [0usize; NROOTS];
    for x in s.iter_mut() {
        *x = data[0] as usize;
    }
    for &d in &data[1..] {
        for (i, x) in s.iter_mut().enumerate() {
            *x = if *x == 0 { d as usize } else { d as usize ^ t.alpha_to[modnn(t.index_of[*x] + (FCR + i) * PRIM)] as usize };
        }
    }
    let mut syn_error = 0;
    for x in s.iter_mut() {
        syn_error |= *x;
        *x = t.index_of[*x];
    }
    if syn_error == 0 {
        return Outcome::Clean;
    }
    // Berlekamp-Massey.
    let mut lambda = [0usize; NROOTS + 1];
    lambda[0] = 1;
    let mut b = [0usize; NROOTS + 1];
    for i in 0..=NROOTS {
        b[i] = t.index_of[lambda[i]];
    }
    let mut el = 0usize;
    let mut tt = [0usize; NROOTS + 1];
    for r in 1..=NROOTS {
        let mut discr_r = 0usize;
        for i in 0..r {
            if lambda[i] != 0 && s[r - i - 1] != A0 {
                discr_r ^= t.alpha_to[modnn(t.index_of[lambda[i]] + s[r - i - 1])] as usize;
            }
        }
        let discr_r = t.index_of[discr_r];
        if discr_r == A0 {
            b.copy_within(0..NROOTS, 1);
            b[0] = A0;
        } else {
            tt[0] = lambda[0];
            for i in 0..NROOTS {
                tt[i + 1] = lambda[i + 1] ^ if b[i] != A0 { t.alpha_to[modnn(discr_r + b[i])] as usize } else { 0 };
            }
            if 2 * el < r {
                el = r - el;
                for i in 0..=NROOTS {
                    b[i] = if lambda[i] == 0 { A0 } else { modnn(t.index_of[lambda[i]] + NN - discr_r) };
                }
            } else {
                b.copy_within(0..NROOTS, 1);
                b[0] = A0;
            }
            lambda = tt;
        }
    }
    let mut deg_lambda = 0;
    for i in 0..=NROOTS {
        lambda[i] = t.index_of[lambda[i]];
        if lambda[i] != A0 {
            deg_lambda = i;
        }
    }
    // Chien search.
    let mut reg = [0usize; NROOTS + 1];
    reg[1..].copy_from_slice(&lambda[1..]);
    let mut root = Vec::new();
    let mut loc = Vec::new();
    let mut k = IPRIM - 1;
    for i in 1..=NN {
        let mut q = 1usize;
        for j in (1..=deg_lambda).rev() {
            if reg[j] != A0 {
                reg[j] = modnn(reg[j] + j);
                q ^= t.alpha_to[reg[j]] as usize;
            }
        }
        if q == 0 {
            root.push(i);
            loc.push(k);
            if root.len() == deg_lambda {
                break;
            }
        }
        k = modnn(k + IPRIM);
    }
    if deg_lambda != root.len() {
        return Outcome::Uncorrectable;
    }
    // Omega = S(x) * Lambda(x) mod x^NROOTS.
    let deg_omega = deg_lambda - 1;
    let mut omega = [A0; NROOTS + 1];
    for i in 0..=deg_omega {
        let mut tmp = 0usize;
        for j in (0..=i).rev() {
            if s[i - j] != A0 && lambda[j] != A0 {
                tmp ^= t.alpha_to[modnn(s[i - j] + lambda[j])] as usize;
            }
        }
        omega[i] = t.index_of[tmp];
    }
    // Forney.
    let mut fixed = Vec::new();
    for j in (0..root.len()).rev() {
        let mut num1 = 0usize;
        for i in (0..=deg_omega).rev() {
            if omega[i] != A0 {
                num1 ^= t.alpha_to[modnn(omega[i] + i * root[j])] as usize;
            }
        }
        let num2 = t.alpha_to[modnn(root[j] * (FCR + NN - 1) + NN)] as usize;
        let mut den = 0usize;
        let top = deg_lambda.min(NROOTS - 1) & !1;
        let mut i = top as isize;
        while i >= 0 {
            let iu = i as usize;
            if lambda[iu + 1] != A0 {
                den ^= t.alpha_to[modnn(lambda[iu + 1] + iu * root[j])] as usize;
            }
            i -= 2;
        }
        if den == 0 {
            return Outcome::Uncorrectable;
        }
        if num1 != 0 {
            if loc[j] < pad {
                return Outcome::Uncorrectable;
            }
            let at = loc[j] - pad;
            data[at] ^= t.alpha_to[modnn(t.index_of[num1] + t.index_of[num2] + NN - t.index_of[den])];
            fixed.push(at);
        }
    }
    for (w, d) in word.iter_mut().zip(&data) {
        *w = t.tal[*d as usize];
    }
    fixed.sort_unstable();
    Outcome::Corrected(fixed)
}

/// A code block interleaved to depth `depth`: symbol `k` belongs to code
/// word `k % depth`. Decodes every code word in place, with `pad` symbols
/// of virtual fill in each; returns each code word's outcome, positions
/// given in the block.
pub fn decode_block(block: &mut [u8], depth: usize, pad: usize) -> Vec<Outcome> {
    let n = NN - pad;
    if depth == 0 || block.len() != n * depth {
        return vec![Outcome::Uncorrectable; depth.max(1)];
    }
    let mut out = Vec::new();
    for w in 0..depth {
        let mut word: Vec<u8> = (0..n).map(|k| block[k * depth + w]).collect();
        let r = decode(&mut word, pad);
        for k in 0..n {
            block[k * depth + w] = word[k];
        }
        out.push(match r {
            Outcome::Corrected(at) => Outcome::Corrected(at.into_iter().map(|k| k * depth + w).collect()),
            o => o,
        });
    }
    out
}

/// Check symbols for a whole interleaved block of data (the first
/// `(223 - pad) * depth` symbols), laid out after it.
pub fn encode_block(data: &[u8], depth: usize, pad: usize) -> Vec<u8> {
    let k = NN - NROOTS - pad;
    let mut parity = vec![0u8; NROOTS * depth];
    for w in 0..depth {
        let word: Vec<u8> = (0..k).map(|i| data[i * depth + w]).collect();
        for (i, p) in encode(&word, pad).into_iter().enumerate() {
            parity[i * depth + w] = p;
        }
    }
    parity
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reproducible byte sequence (no rand dependency).
    fn noise(seed: u32, n: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2654435761).max(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn the_dual_basis_is_a_permutation_with_one_at_0x7b() {
        let t = tables();
        assert_eq!(t.tal[1], 0x7b);
        let mut seen = [false; 256];
        for i in 0..256 {
            seen[t.tal[i] as usize] = true;
            assert_eq!(t.tal1[t.tal[i] as usize] as usize, i);
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn up_to_sixteen_symbol_errors_are_corrected() {
        for (seed, errors) in [(1u32, 0usize), (2, 1), (3, 8), (4, 16)] {
            let data = noise(seed, 223);
            let mut word = data.clone();
            word.extend(encode(&data, 0));
            let clean = word.clone();
            let positions: Vec<usize> = noise(seed + 100, errors).into_iter().enumerate().map(|(i, b)| (b as usize + i * 15) % 255).collect();
            let mut unique = positions.clone();
            unique.sort_unstable();
            unique.dedup();
            for &p in &unique {
                word[p] ^= 0x5A;
            }
            let r = decode(&mut word, 0);
            assert_eq!(word, clean, "seed {seed}");
            match r {
                Outcome::Clean => assert!(unique.is_empty()),
                Outcome::Corrected(at) => assert_eq!(at, unique),
                Outcome::Uncorrectable => panic!("{errors} errors should correct"),
            }
        }
    }

    #[test]
    fn seventeen_errors_are_not_miscorrected_silently_and_virtual_fill_works() {
        let data = noise(9, 223);
        let mut word = data.clone();
        word.extend(encode(&data, 0));
        for p in 0..17 {
            word[p * 7] ^= 0xFF;
        }
        let before = word.clone();
        if decode(&mut word, 0) == Outcome::Uncorrectable {
            assert_eq!(word, before, "left as it was");
        }
        // Shortened: 223 - 60 data symbols.
        let short = noise(10, 163);
        let mut w = short.clone();
        w.extend(encode(&short, 60));
        let clean = w.clone();
        w[3] ^= 1;
        w[170] ^= 0x80;
        assert_eq!(decode(&mut w, 60), Outcome::Corrected(vec![3, 170]));
        assert_eq!(w, clean);
    }

    #[test]
    fn an_interleaved_block_corrects_each_code_word() {
        let depth = 5;
        let data = noise(11, 223 * depth);
        let mut block = data.clone();
        block.extend(encode_block(&data, depth, 0));
        let clean = block.clone();
        for p in [0, 1, 2, 3, 4, 500, 1000] {
            block[p] ^= 0x33;
        }
        let r = decode_block(&mut block, depth, 0);
        assert_eq!(block, clean);
        assert!(r.iter().all(|o| matches!(o, Outcome::Corrected(_))), "{r:?}");
    }
}
