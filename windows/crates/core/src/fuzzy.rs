//! Fuzzy file-name matching for Go to File: characters must appear in order, and matches at word starts,
//! camelCase humps and inside the file name score higher.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i64,
    /// Matched byte offsets into the candidate.
    pub positions: Vec<usize>,
}

#[inline(always)]
fn lower(c: u8) -> u8 {
    c.to_ascii_lowercase()
}

#[inline(always)]
fn is_separator(c: u8) -> bool {
    matches!(c, b'/' | b'_' | b'-' | b'.' | b' ')
}

/// Scores `candidate` against a lowercased query. None when the query is not a subsequence.
pub fn match_path(query: &[u8], candidate: &str, want_positions: bool) -> Option<Match> {
    let buf = candidate.as_bytes();
    let (n, m) = (buf.len(), query.len());
    if m == 0 || n < m {
        return None;
    }
    // Cheap subsequence rejection before any scoring.
    let mut qi = 0;
    for &b in buf {
        if lower(b) == query[qi] {
            qi += 1;
            if qi == m {
                break;
            }
        }
    }
    if qi != m {
        return None;
    }
    let name_start = buf.iter().rposition(|&b| b == b'/').map(|i| i + 1).unwrap_or(0);

    // Prefer matching entirely inside the file name; fall back to the whole path.
    if let Some(mut in_name) = score(buf, name_start, query, want_positions) {
        in_name.score += 1000 - (n - name_start).min(400) as i64; // shorter names win ties
        return Some(in_name);
    }
    let mut whole = score(buf, 0, query, want_positions)?;
    whole.score -= n.min(300) as i64;
    Some(whole)
}

/// For each query character, the best-scoring position given the best placement of the characters before it.
fn score(buf: &[u8], from: usize, query: &[u8], want_positions: bool) -> Option<Match> {
    let (n, m) = (buf.len(), query.len());
    if n - from < m {
        return None;
    }
    let width = n - from;
    const NEGATIVE: i64 = i64::MIN / 4;
    let mut previous = vec![NEGATIVE; width];
    let mut current = vec![NEGATIVE; width];
    let mut backtrack: Vec<Vec<i32>> = if want_positions { vec![vec![-1; width]; m] } else { Vec::new() };

    let bonus = |j: usize| -> i64 {
        let i = from + j;
        if i == from {
            return 40; // start of the name
        }
        let prev = buf[i - 1];
        if is_separator(prev) {
            return 30; // after / _ - .
        }
        if buf[i].is_ascii_uppercase() && !prev.is_ascii_uppercase() {
            return 28; // camelCase hump
        }
        0
    };

    for (qi, &q) in query.iter().enumerate() {
        let mut best_prev = NEGATIVE; // best of previous[0..j-1]
        let mut best_prev_index: i32 = -1;
        for j in 0..width {
            current[j] = NEGATIVE;
            if j >= qi && lower(buf[from + j]) == q {
                if qi == 0 {
                    current[j] = 10 + bonus(j) - j.min(20) as i64;
                } else {
                    // Either continue directly from the previous character (consecutive bonus) or jump.
                    let mut best = NEGATIVE;
                    let mut best_from: i32 = -1;
                    if j > 0 && previous[j - 1] > NEGATIVE {
                        best = previous[j - 1] + 10 + 35;
                        best_from = (j - 1) as i32;
                    }
                    if best_prev > NEGATIVE {
                        let jump = best_prev + 10 + bonus(j) - 2;
                        if jump > best {
                            best = jump;
                            best_from = best_prev_index;
                        }
                    }
                    if best_from >= 0 {
                        current[j] = best;
                        if want_positions {
                            backtrack[qi][j] = best_from;
                        }
                    }
                }
            }
            // Make previous[j-1] available to the *next* j as a non-consecutive jump source.
            if j > 0 && qi > 0 && previous[j - 1] > best_prev {
                best_prev = previous[j - 1];
                best_prev_index = (j - 1) as i32;
            }
        }
        std::mem::swap(&mut previous, &mut current);
    }

    let mut best = NEGATIVE;
    let mut best_index: i32 = -1;
    for (j, &v) in previous.iter().enumerate() {
        if v > best {
            best = v;
            best_index = j as i32;
        }
    }
    if best_index < 0 {
        return None;
    }
    let mut positions = Vec::new();
    if want_positions {
        let mut j = best_index;
        let mut qi = m as i32 - 1;
        while qi >= 0 && j >= 0 {
            positions.push(from + j as usize);
            j = backtrack[qi as usize][j as usize];
            qi -= 1;
        }
        positions.reverse();
    }
    Some(Match { score: best, positions })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub path: String,
    pub score: i64,
    pub positions: Vec<usize>,
}

/// Top `limit` matches for `query` among `paths`, best first.
pub fn rank(query: &str, paths: &[String], limit: usize, is_cancelled: impl Fn() -> bool) -> Vec<Ranked> {
    let q: Vec<u8> = query.to_lowercase().bytes().filter(|&b| b != b' ').collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i64, usize)> = Vec::with_capacity(1024);
    for (index, path) in paths.iter().enumerate() {
        if index & 0x3FF == 0 && is_cancelled() {
            return Vec::new();
        }
        if let Some(m) = match_path(&q, path, false) {
            scored.push((m.score, index));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| paths[a.1].len().cmp(&paths[b.1].len())));
    scored
        .into_iter()
        .take(limit)
        .filter_map(|(_, index)| {
            let path = &paths[index];
            let m = match_path(&q, path, true)?;
            Some(Ranked { path: path.clone(), score: m.score, positions: m.positions })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_name_matches_first() {
        let paths: Vec<String> = [
            "app/src/main/java/com/example/MainActivity.kt",
            "docs/manual/activity.md",
            "app/src/main/res/layout/activity_main.xml",
            "build.gradle",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let ranked = rank("mainact", &paths, 10, || false);
        assert_eq!(ranked[0].path, "app/src/main/java/com/example/MainActivity.kt");
        let name = &ranked[0].path;
        let picked: String = ranked[0].positions.iter().map(|&p| name.as_bytes()[p] as char).collect();
        assert_eq!(picked.to_lowercase(), "mainact");
        assert!(rank("zzz", &paths, 10, || false).is_empty());
        assert_eq!(rank("bg", &paths, 10, || false)[0].path, "build.gradle");
    }
}
