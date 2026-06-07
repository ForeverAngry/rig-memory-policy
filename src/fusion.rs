//! Reciprocal Rank Fusion (RRF) for combining ranked result lists.
//!
//! Hybrid retrieval — the current state of the art — runs several retrievers
//! (dense vectors, lexical/BM25, graph traversal) and merges their ranked
//! outputs. Reciprocal Rank Fusion is the standard rank-only merge: it needs
//! no score calibration across retrievers, only each item's *position* in each
//! list. An item's fused score is the sum over lists of `1 / (k + rank)`,
//! where `rank` is 0-based and `k` damps the influence of low ranks
//! (`k = 60` is the conventional default from the original RRF paper).
//!
//! This module is pure and dependency-free: backends pass in already-ranked
//! id lists and receive a single deterministically-ordered fused ranking.
//!
//! # Example
//!
//! ```
//! use rig_memory_policy::fusion::{reciprocal_rank_fusion, RRF_DEFAULT_K};
//!
//! // Two retrievers rank documents; "a" tops both lists.
//! let dense = ["a", "b", "c"];
//! let lexical = ["a", "c", "d"];
//!
//! let fused = reciprocal_rank_fusion([dense.as_slice(), lexical.as_slice()], RRF_DEFAULT_K);
//! // "a" is rank 0 in both lists, so it leads; "c" appears high in both too.
//! assert_eq!(fused[0].0, "a");
//! assert_eq!(fused[1].0, "c");
//! ```

use std::collections::HashMap;
use std::hash::Hash;

/// The conventional RRF damping constant from the original publication.
pub const RRF_DEFAULT_K: f64 = 60.0;

/// Fuse several ranked lists into one ranking via Reciprocal Rank Fusion.
///
/// Each input list is a slice of item ids in descending rank order (best
/// first). The returned vector pairs each distinct item with its fused score,
/// sorted by descending score. Ties are broken deterministically by the order
/// in which items were first encountered across the input lists, so the output
/// is stable regardless of hash-map iteration order.
///
/// `k` damps the contribution of lower ranks; pass [`RRF_DEFAULT_K`] for the
/// standard behaviour. A non-positive `k` is clamped to a tiny positive value
/// to avoid division by zero.
#[must_use]
pub fn reciprocal_rank_fusion<I, L, T>(lists: I, k: f64) -> Vec<(T, f64)>
where
    I: IntoIterator<Item = L>,
    L: AsRef<[T]>,
    T: Eq + Hash + Clone,
{
    let k = if k > 0.0 { k } else { f64::EPSILON };

    // Preserve first-seen order for deterministic tie-breaking.
    let mut order: Vec<T> = Vec::new();
    let mut index: HashMap<T, usize> = HashMap::new();
    let mut scores: Vec<f64> = Vec::new();

    for list in lists {
        for (rank, item) in list.as_ref().iter().enumerate() {
            let contribution = 1.0 / (k + rank as f64);
            match index.get(item) {
                Some(&slot) => {
                    if let Some(score) = scores.get_mut(slot) {
                        *score += contribution;
                    }
                }
                None => {
                    let slot = order.len();
                    index.insert(item.clone(), slot);
                    order.push(item.clone());
                    scores.push(contribution);
                }
            }
        }
    }

    let mut fused: Vec<(usize, T, f64)> = order
        .into_iter()
        .enumerate()
        .map(|(slot, item)| (slot, item, scores.get(slot).copied().unwrap_or(0.0)))
        .collect();

    // Descending score; stable tie-break on first-seen slot.
    fused.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    fused
        .into_iter()
        .map(|(_, item, score)| (item, score))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn fuses_two_lists_by_combined_rank() {
        let dense = ["a", "b", "c"];
        let lexical = ["a", "c", "d"];
        let fused = reciprocal_rank_fusion([dense.as_slice(), lexical.as_slice()], RRF_DEFAULT_K);

        // "a": 1/60 + 1/60 leads; "c": 1/62 + 1/61 is second.
        assert_eq!(fused[0].0, "a");
        assert_eq!(fused[1].0, "c");
        assert!(fused[0].1 > fused[1].1);
        // All four distinct ids appear exactly once.
        assert_eq!(fused.len(), 4);
    }

    #[test]
    fn exact_tie_breaks_on_first_seen_order() {
        // Symmetric inputs give "a" and "b" identical fused scores; the item
        // encountered first wins the tie deterministically.
        let l1 = ["a", "b"];
        let l2 = ["b", "a"];
        let fused = reciprocal_rank_fusion([l1.as_slice(), l2.as_slice()], RRF_DEFAULT_K);
        assert!((fused[0].1 - fused[1].1).abs() < 1e-12);
        assert_eq!(fused[0].0, "a");
    }

    #[test]
    fn item_in_both_lists_outranks_item_in_one() {
        let l1 = ["x", "solo"];
        let l2 = ["x", "other"];
        let fused = reciprocal_rank_fusion([l1.as_slice(), l2.as_slice()], RRF_DEFAULT_K);
        assert_eq!(fused[0].0, "x");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let fused: Vec<(&str, f64)> = reciprocal_rank_fusion(Vec::<&[&str]>::new(), RRF_DEFAULT_K);
        assert!(fused.is_empty());
    }

    #[test]
    fn non_positive_k_does_not_divide_by_zero() {
        let l1 = ["a", "b"];
        let fused = reciprocal_rank_fusion([l1.as_slice()], 0.0);
        assert_eq!(fused.len(), 2);
        assert!(fused.iter().all(|(_, score)| score.is_finite()));
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        let l1 = ["a", "b", "c"];
        let l2 = ["c", "b", "a"];
        let first = reciprocal_rank_fusion([l1.as_slice(), l2.as_slice()], RRF_DEFAULT_K);
        let second = reciprocal_rank_fusion([l1.as_slice(), l2.as_slice()], RRF_DEFAULT_K);
        assert_eq!(first, second);
    }
}
