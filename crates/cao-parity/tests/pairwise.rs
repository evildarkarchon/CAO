//! The deterministic pairwise covering array the corpus generator draws its
//! cases from (#473).

use cao_parity::pairwise::{Level, covering_array};

/// Every pair of levels of two different parameters, as `(i, a, j, b)` with
/// `i < j`.
fn all_pairs(levels: &[usize]) -> Vec<(usize, usize, usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..levels.len() {
        for j in i + 1..levels.len() {
            for a in 0..levels[i] {
                for b in 0..levels[j] {
                    pairs.push((i, a, j, b));
                }
            }
        }
    }
    pairs
}

fn covers(rows: &[Vec<Level>], (i, a, j, b): (usize, usize, usize, usize)) -> bool {
    rows.iter().any(|row| row[i] == a && row[j] == b)
}

#[test]
fn three_binary_parameters_are_covered_in_four_rows() {
    // The known optimum for three binary parameters is four rows.
    let rows = covering_array(&[2, 2, 2], &|_, _, _, _| true).unwrap();
    assert_eq!(rows.len(), 4, "{rows:?}");
    for pair in all_pairs(&[2, 2, 2]) {
        assert!(covers(&rows, pair), "{pair:?} in {rows:?}");
    }
}

#[test]
fn every_pair_of_a_mixed_array_is_covered_and_no_row_repeats_work() {
    let levels = [3, 2, 4, 2, 2, 3, 2];
    let rows = covering_array(&levels, &|_, _, _, _| true).unwrap();
    for pair in all_pairs(&levels) {
        assert!(covers(&rows, pair), "{pair:?}");
    }
    // 4 x 3 = 12 is the lower bound; a greedy array stays near it, far below
    // the 576 rows of the full product.
    assert!(rows.len() >= 12 && rows.len() <= 20, "{} rows", rows.len());
    for row in &rows {
        assert_eq!(row.len(), levels.len());
        assert!(row.iter().zip(&levels).all(|(level, count)| level < count));
    }
}

#[test]
fn a_forbidden_pair_never_appears_and_every_other_pair_is_covered() {
    // Parameter 0 level 1 forbids parameter 2 level 1, as FO4 forbids Mesh work.
    let forbidden =
        |i: usize, a: Level, j: usize, b: Level| !(i == 0 && a == 1 && j == 2 && b == 1);
    let levels = [2, 3, 2, 2];
    let rows = covering_array(&levels, &forbidden).unwrap();
    for row in &rows {
        assert!(!(row[0] == 1 && row[2] == 1), "{row:?}");
    }
    for pair in all_pairs(&levels) {
        if pair != (0, 1, 2, 1) {
            assert!(covers(&rows, pair), "{pair:?}");
        }
    }
}

#[test]
fn the_same_inputs_give_the_same_array() {
    let levels = [3, 2, 4, 2, 6, 3];
    let allowed = |i: usize, a: Level, j: usize, b: Level| !(i == 1 && a == 1 && j == 4 && b > 2);
    let first = covering_array(&levels, &allowed).unwrap();
    for _ in 0..3 {
        assert_eq!(covering_array(&levels, &allowed).unwrap(), first);
    }
}

#[test]
fn a_pair_no_row_can_complete_is_an_error() {
    // Parameter 0's level 0 is allowed with parameter 1, but parameter 2 has
    // no level it allows, so the pair (0=0, 1=0) can never sit in a row.
    let allowed = |i: usize, a: Level, j: usize, _b: Level| !(i == 0 && a == 0 && j == 2);
    assert!(covering_array(&[2, 2, 2], &allowed).is_err());
}
