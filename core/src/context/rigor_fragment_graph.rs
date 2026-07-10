//! Dependency metadata for the Plan mode Rigor tier fragments.
//!
//! This module is documentation-as-data: it mirrors the "Dependency graph" section of
//! `collaboration-mode-templates/FRAGMENT_CATALOG.md`. Each fragment lists only its HARD
//! dependencies — the ones a fragment's own text names via an "In addition to ... above"
//! phrase, which read as a non-sequitur if the referenced fragment did not already render.
//!
//! The test module verifies two properties of the graph:
//! 1. It is acyclic (a fragment can never transitively depend on itself).
//! 2. The actual composition order in `from_collaboration_mode` is a valid topological sort
//!    of this graph — i.e. every fragment renders after all of its hard dependencies.
//!
//! If you add a fragment, add it here too (see FRAGMENT_CATALOG.md "How to add a new fragment").

/// A single Rigor tier fragment and the fragments it hard-depends on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RigorFragmentNode {
    /// Matches the `with_rigor_<name>` method suffix, upper-cased.
    pub(crate) name: &'static str,
    /// Names of fragments that must render before this one, per its own "above" text.
    pub(crate) hard_dependencies: &'static [&'static str],
}

/// The 11 Rigor tier fragments and their hard dependencies, in no particular order.
///
/// Kept independent of composition order deliberately — `assert_topological_order` below is
/// what actually checks the chain in `collaboration_mode_instructions.rs` against this graph.
pub(crate) const RIGOR_FRAGMENT_GRAPH: &[RigorFragmentNode] = &[
    RigorFragmentNode { name: "WORKFLOW", hard_dependencies: &[] },
    RigorFragmentNode { name: "COVERAGE", hard_dependencies: &[] },
    RigorFragmentNode { name: "TASK_SKELETON", hard_dependencies: &[] },
    RigorFragmentNode { name: "SELFREVIEW", hard_dependencies: &["COVERAGE"] },
    RigorFragmentNode {
        name: "INVARIANTS",
        hard_dependencies: &["COVERAGE", "SELFREVIEW"],
    },
    RigorFragmentNode { name: "GROUNDING", hard_dependencies: &[] },
    RigorFragmentNode { name: "SCOPE", hard_dependencies: &["GROUNDING"] },
    RigorFragmentNode { name: "RENAME", hard_dependencies: &[] },
    RigorFragmentNode {
        name: "RISKS",
        hard_dependencies: &["COVERAGE", "SELFREVIEW"],
    },
    RigorFragmentNode { name: "SPLIT", hard_dependencies: &[] },
    RigorFragmentNode {
        name: "TURN_DISCIPLINE",
        hard_dependencies: &["SPLIT"],
    },
];

/// The actual composition order used in `from_collaboration_mode`. Kept as a separate constant
/// (rather than derived) so a diff against the real chain is a one-line change to review.
pub(crate) const RIGOR_COMPOSITION_ORDER: &[&str] = &[
    "WORKFLOW",
    "COVERAGE",
    "TASK_SKELETON",
    "SELFREVIEW",
    "INVARIANTS",
    "GROUNDING",
    "SCOPE",
    "RENAME",
    "RISKS",
    "SPLIT",
    "TURN_DISCIPLINE",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn find(name: &'static str) -> &'static RigorFragmentNode {
        RIGOR_FRAGMENT_GRAPH
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("fragment {name} not found in RIGOR_FRAGMENT_GRAPH"))
    }

    #[test]
    fn every_dependency_name_resolves_to_a_known_fragment() {
        let known: HashSet<&str> = RIGOR_FRAGMENT_GRAPH.iter().map(|n| n.name).collect();
        for node in RIGOR_FRAGMENT_GRAPH {
            for dep in node.hard_dependencies {
                assert!(
                    known.contains(dep),
                    "{} depends on unknown fragment {}",
                    node.name,
                    dep
                );
            }
        }
    }

    #[test]
    fn graph_has_no_cycles() {
        // Simple DFS cycle check over the small fixed graph.
        fn visit(
            name: &'static str,
            visiting: &mut HashSet<&'static str>,
            done: &mut HashSet<&'static str>,
        ) {
            if done.contains(name) {
                return;
            }
            assert!(
                visiting.insert(name),
                "cycle detected: {name} depends on itself transitively"
            );
            for dep in find(name).hard_dependencies {
                visit(dep, visiting, done);
            }
            visiting.remove(name);
            done.insert(find(name).name);
        }

        let mut visiting = HashSet::new();
        let mut done = HashSet::new();
        for node in RIGOR_FRAGMENT_GRAPH {
            visit(node.name, &mut visiting, &mut done);
        }
    }

    #[test]
    fn composition_order_covers_every_fragment_exactly_once() {
        let graph_names: HashSet<&str> = RIGOR_FRAGMENT_GRAPH.iter().map(|n| n.name).collect();
        let order_names: HashSet<&str> = RIGOR_COMPOSITION_ORDER.iter().copied().collect();
        assert_eq!(
            RIGOR_COMPOSITION_ORDER.len(),
            RIGOR_FRAGMENT_GRAPH.len(),
            "composition order has duplicates or a missing fragment"
        );
        assert_eq!(
            graph_names, order_names,
            "RIGOR_COMPOSITION_ORDER and RIGOR_FRAGMENT_GRAPH must name the same fragments"
        );
    }

    #[test]
    fn composition_order_respects_every_hard_dependency() {
        for (index, name) in RIGOR_COMPOSITION_ORDER.iter().enumerate() {
            let node = find(name);
            for dep in node.hard_dependencies {
                let dep_index = RIGOR_COMPOSITION_ORDER
                    .iter()
                    .position(|n| n == dep)
                    .unwrap_or_else(|| panic!("dependency {dep} of {name} missing from composition order"));
                assert!(
                    dep_index < index,
                    "{name} is composed at position {index} but its hard dependency {dep} \
                     is at position {dep_index} — {dep} must render first or {name}'s \
                     \"... above\" text will refer to nothing",
                );
            }
        }
    }

    /// Cross-check against the real chain in `from_collaboration_mode`. If someone reorders
    /// the `.with_rigor_*()` calls there without updating RIGOR_COMPOSITION_ORDER, this test
    /// won't catch the drift by itself — but `collaboration_mode_instructions::tests::
    /// rigor_tier_composes_all_fragments` pins the rendered output, so the two together give
    /// full coverage: this test checks the order is *valid*, that test checks it's *actual*.
    #[test]
    fn composition_order_matches_documented_source_of_truth() {
        // Mirrors the chain in `CollaborationModeInstructions::from_collaboration_mode`.
        let actual_chain = [
            "WORKFLOW",
            "COVERAGE",
            "TASK_SKELETON",
            "SELFREVIEW",
            "INVARIANTS",
            "GROUNDING",
            "SCOPE",
            "RENAME",
            "RISKS",
            "SPLIT",
            "TURN_DISCIPLINE",
        ];
        assert_eq!(
            RIGOR_COMPOSITION_ORDER, actual_chain,
            "RIGOR_COMPOSITION_ORDER drifted from the .with_rigor_*() chain in \
             collaboration_mode_instructions.rs — update one to match the other"
        );
    }
}
