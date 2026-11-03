//! How many tests a task's run passed and failed, read from its output:
//! cargo's `test result:` lines (one per test binary, added up), the
//! summary line pytest, Jest and Vitest end with, and `go test -v`'s
//! `--- PASS` and `--- FAIL` lines. None when the output says neither.

/// Tests passed and failed in `output`, or none when it reports no tests
/// Fenix recognises.
pub fn test_counts(output: &str) -> Option<(u32, u32)> {
    let mut cargo: Option<(u32, u32)> = None;
    let mut summary: Option<(u32, u32)> = None;
    let mut go = (0u32, 0u32);
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("test result:") {
            let (p, f) = (count_before(rest, "passed").unwrap_or(0), count_before(rest, "failed").unwrap_or(0));
            let c = cargo.get_or_insert((0, 0));
            c.0 += p;
            c.1 += f;
        } else if line.starts_with("--- PASS") {
            go.0 += 1;
        } else if line.starts_with("--- FAIL") {
            go.1 += 1;
        } else if is_summary(line) {
            let (p, f) = (count_before(line, "passed"), count_before(line, "failed"));
            if p.is_some() || f.is_some() {
                summary = Some((p.unwrap_or(0), f.unwrap_or(0)));
            }
        }
    }
    cargo.or(summary).or((go != (0, 0)).then_some(go))
}

/// pytest's `==== 2 failed, 10 passed in 0.31s ====`, Jest's `Tests: 1
/// failed, 9 passed, 10 total`, Vitest's `Tests  1 failed | 9 passed (10)`.
fn is_summary(line: &str) -> bool {
    let pytest = line.starts_with('=') && line.contains(" in ") && (line.contains(" passed") || line.contains(" failed"));
    let jest_or_vitest = line.starts_with("Tests") && (line.contains(" passed") || line.contains(" failed"));
    pytest || jest_or_vitest
}

/// The number written just before `word` in `text`: `10 passed` gives 10.
fn count_before(text: &str, word: &str) -> Option<u32> {
    let at = text.find(word)?;
    let before = text[..at].trim_end();
    let digits: String = before.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_adds_up_each_binary() {
        let out = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n\nrunning 5 tests\ntest result: FAILED. 4 passed; 1 failed; 0 ignored\n";
        assert_eq!(test_counts(out), Some((7, 1)));
    }

    #[test]
    fn pytest_jest_and_vitest_summaries() {
        assert_eq!(test_counts("==== 2 failed, 10 passed in 0.31s ===="), Some((10, 2)));
        assert_eq!(test_counts("========== 12 passed in 1.02s =========="), Some((12, 0)));
        assert_eq!(test_counts("Tests:       1 failed, 9 passed, 10 total\nTime: 2s"), Some((9, 1)));
        assert_eq!(test_counts(" Tests  1 failed | 9 passed (10)"), Some((9, 1)));
    }

    #[test]
    fn go_test_verbose() {
        let out = "=== RUN   TestA\n--- PASS: TestA (0.00s)\n=== RUN   TestB\n--- FAIL: TestB (0.00s)\n--- PASS: TestC (0.00s)\nFAIL";
        assert_eq!(test_counts(out), Some((2, 1)));
    }

    #[test]
    fn a_build_reports_no_tests() {
        assert_eq!(test_counts("   Compiling foo v0.1.0\n    Finished `dev` profile"), None);
        assert_eq!(test_counts("all 3 tests passed, said someone"), None);
    }
}
