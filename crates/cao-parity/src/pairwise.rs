//! A deterministic pairwise covering array (#473).
//!
//! Every pair of levels of two different parameters that the constraints allow
//! appears in at least one row. The builder is greedy: each row starts from the
//! first pair still uncovered, then gives every other parameter the level that
//! covers the most new pairs, the lowest level winning a tie. Nothing is
//! random and nothing is iterated in hash order, so the same parameters and
//! constraints always give the same rows.

use std::collections::BTreeSet;

/// A parameter's level, as an index into its levels.
pub type Level = usize;

/// The pairwise constraint: whether level `a` of parameter `i` may share a row
/// with level `b` of parameter `j`. It is only asked with `i < j`.
pub type Allowed<'a> = dyn Fn(usize, Level, usize, Level) -> bool + 'a;

/// One pair, as `(i, a, j, b)` with `i < j`.
type Pair = (usize, Level, usize, Level);

/// Builds the rows of a pairwise covering array.
///
/// `levels[i]` is the number of levels of parameter `i`. Each row holds one
/// level per parameter, and every row satisfies `allowed` for each of its
/// pairs. Pairs `allowed` forbids are never required.
///
/// # Errors
/// A message naming the pair when an allowed pair cannot be completed into a
/// row: some other parameter has no level both of its levels allow.
pub fn covering_array(levels: &[usize], allowed: &Allowed<'_>) -> Result<Vec<Vec<Level>>, String> {
    let ordered = |i: usize, a: Level, j: usize, b: Level| -> Pair {
        if i < j { (i, a, j, b) } else { (j, b, i, a) }
    };
    let compatible = |i: usize, a: Level, j: usize, b: Level| {
        let (i, a, j, b) = ordered(i, a, j, b);
        allowed(i, a, j, b)
    };

    let mut uncovered: BTreeSet<Pair> = BTreeSet::new();
    for i in 0..levels.len() {
        for j in i + 1..levels.len() {
            for a in 0..levels[i] {
                for b in 0..levels[j] {
                    if allowed(i, a, j, b) {
                        uncovered.insert((i, a, j, b));
                    }
                }
            }
        }
    }

    let mut rows = Vec::new();
    while let Some(&(i, a, j, b)) = uncovered.first() {
        let mut row: Vec<Option<Level>> = vec![None; levels.len()];
        row[i] = Some(a);
        row[j] = Some(b);
        // Whether `k = c` fits the row so far and still leaves every
        // unassigned parameter a level that fits it too. Looking one step
        // ahead keeps a greedy choice from stranding a later parameter.
        let fits = |row: &[Option<Level>], k: usize, c: Level| {
            let fits_assigned = |row: &[Option<Level>], k: usize, c: Level| {
                row.iter().enumerate().all(|(m, level)| {
                    m == k || level.is_none_or(|level| compatible(k, c, m, level))
                })
            };
            if !fits_assigned(row, k, c) {
                return false;
            }
            (0..levels.len())
                .filter(|&m| m != k && row[m].is_none())
                .all(|m| (0..levels[m]).any(|d| compatible(m, d, k, c) && fits_assigned(row, m, d)))
        };
        let stranded = || format!("the pair {i}={a}, {j}={b} cannot be completed into a row");
        if !(fits(&row, i, a) && fits(&row, j, b)) {
            return Err(stranded());
        }
        for k in 0..levels.len() {
            if row[k].is_some() {
                continue;
            }
            let mut best: Option<(Level, usize)> = None;
            for c in 0..levels[k] {
                if !fits(&row, k, c) {
                    continue;
                }
                let gain = row
                    .iter()
                    .enumerate()
                    .filter_map(|(m, level)| level.map(|level| (m, level)))
                    .filter(|&(m, level)| uncovered.contains(&ordered(k, c, m, level)))
                    .count();
                // Strictly greater, so the lowest level wins a tie.
                if best.is_none_or(|(_, most)| gain > most) {
                    best = Some((c, gain));
                }
            }
            row[k] = Some(best.ok_or_else(stranded)?.0);
        }
        let row: Vec<Level> = row
            .into_iter()
            .map(|level| level.expect("every parameter was given a level above"))
            .collect();
        for p in 0..row.len() {
            for q in p + 1..row.len() {
                uncovered.remove(&(p, row[p], q, row[q]));
            }
        }
        rows.push(row);
    }
    Ok(rows)
}
