//! Representation-neutral three-valued Boolean evaluation.

use std::collections::HashSet;

use crate::query::binding::filter::BoundExpr;

pub(crate) enum BooleanNode<'a, T> {
    Leaf,
    And(&'a [T]),
    Or(&'a [T]),
    Not(&'a T),
}

pub(crate) trait BooleanTree: Sized {
    fn boolean_node(&self) -> BooleanNode<'_, Self>;
}

impl BooleanTree for BoundExpr {
    fn boolean_node(&self) -> BooleanNode<'_, Self> {
        match self {
            Self::Term(_) => BooleanNode::Leaf,
            Self::And(parts) => BooleanNode::And(parts),
            Self::Or(parts) => BooleanNode::Or(parts),
            Self::Not(inner) => BooleanNode::Not(inner),
        }
    }
}

/// Evaluate Boolean composition with decisive membership exits and no evidence
/// allocation. Leaf evaluation remains the caller's responsibility.
pub(crate) fn eval_membership<T, E, F>(node: &T, leaf: &mut F) -> Result<Option<bool>, E>
where
    T: BooleanTree,
    F: FnMut(&T) -> Result<Option<bool>, E>,
{
    eval(node, None, &mut |node, _| leaf(node), &mut |_| true, true)
}

/// Evaluate Boolean composition while collecting fields from every contributing
/// true leaf. `may_contribute_after_true` lets expression-specific evidence rules
/// skip later `OR` branches without changing truth visitation for other callers.
pub(crate) fn eval_with_evidence<T, E, F, V>(
    node: &T,
    evidence: &mut HashSet<String>,
    leaf: &mut F,
    may_contribute_after_true: &mut V,
    negated_is_membership: bool,
) -> Result<Option<bool>, E>
where
    T: BooleanTree,
    F: FnMut(&T, Option<&mut HashSet<String>>) -> Result<Option<bool>, E>,
    V: FnMut(&T) -> bool,
{
    eval(
        node,
        Some(evidence),
        leaf,
        may_contribute_after_true,
        negated_is_membership,
    )
}

fn eval<T, E, F, V>(
    node: &T,
    mut evidence: Option<&mut HashSet<String>>,
    leaf: &mut F,
    may_contribute_after_true: &mut V,
    negated_is_membership: bool,
) -> Result<Option<bool>, E>
where
    T: BooleanTree,
    F: FnMut(&T, Option<&mut HashSet<String>>) -> Result<Option<bool>, E>,
    V: FnMut(&T) -> bool,
{
    match node.boolean_node() {
        BooleanNode::Leaf => leaf(node, evidence),
        BooleanNode::And(parts) => {
            let mut branch = evidence.as_ref().map(|_| HashSet::new());
            let mut unknown = false;
            for part in parts {
                match eval(
                    part,
                    branch.as_mut(),
                    leaf,
                    may_contribute_after_true,
                    negated_is_membership,
                )? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown {
                Ok(None)
            } else {
                if let (Some(evidence), Some(branch)) = (evidence.as_mut(), branch) {
                    evidence.extend(branch);
                }
                Ok(Some(true))
            }
        }
        BooleanNode::Or(parts) => {
            let mut matched = false;
            let mut unknown = false;
            for part in parts {
                if evidence.is_some() && matched && !may_contribute_after_true(part) {
                    continue;
                }
                let mut branch = evidence.as_ref().map(|_| HashSet::new());
                match eval(
                    part,
                    branch.as_mut(),
                    leaf,
                    may_contribute_after_true,
                    negated_is_membership,
                )? {
                    Some(true) => {
                        matched = true;
                        if evidence.is_none() {
                            return Ok(Some(true));
                        }
                        if let (Some(evidence), Some(branch)) = (evidence.as_mut(), branch) {
                            evidence.extend(branch);
                        }
                    }
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            if matched {
                Ok(Some(true))
            } else if unknown {
                Ok(None)
            } else {
                Ok(Some(false))
            }
        }
        BooleanNode::Not(inner) => {
            let mut suppressed = (!negated_is_membership && evidence.is_some()).then(HashSet::new);
            Ok(eval(
                inner,
                suppressed.as_mut(),
                leaf,
                may_contribute_after_true,
                negated_is_membership,
            )?
            .map(|value| !value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    enum TestTree {
        Leaf(&'static str, Option<bool>),
        And(Vec<TestTree>),
        Or(Vec<TestTree>),
        Not(Box<TestTree>),
    }

    impl BooleanTree for TestTree {
        fn boolean_node(&self) -> BooleanNode<'_, Self> {
            match self {
                Self::Leaf(..) => BooleanNode::Leaf,
                Self::And(parts) => BooleanNode::And(parts),
                Self::Or(parts) => BooleanNode::Or(parts),
                Self::Not(inner) => BooleanNode::Not(inner),
            }
        }
    }

    #[test]
    fn empty_boolean_branches_use_the_documented_identities() {
        let empty_and = TestTree::And(Vec::new());
        let empty_or = TestTree::Or(Vec::new());
        let mut leaf = |_: &TestTree| Ok::<_, ()>(Some(true));
        assert_eq!(eval_membership(&empty_and, &mut leaf).unwrap(), Some(true));
        assert_eq!(eval_membership(&empty_or, &mut leaf).unwrap(), Some(false));
    }

    #[test]
    fn membership_visits_only_the_decisive_true_or_branch() {
        let tree = TestTree::Or(vec![
            TestTree::Leaf("first", Some(true)),
            TestTree::Leaf("second", Some(true)),
            TestTree::Leaf("unavailable", None),
        ]);
        let mut visits = Vec::new();
        let result = eval_membership(&tree, &mut |node| {
            let TestTree::Leaf(name, truth) = node else {
                unreachable!()
            };
            visits.push(*name);
            Ok::<_, ()>(*truth)
        })
        .unwrap();
        assert_eq!(result, Some(true));
        assert_eq!(visits, ["first"]);
    }

    #[test]
    fn evidence_visits_all_or_branches_and_preserves_unknown_negation() {
        let tree = TestTree::Or(vec![
            TestTree::Leaf("first", Some(true)),
            TestTree::Leaf("second", Some(true)),
            TestTree::Not(Box::new(TestTree::Leaf("unavailable", None))),
        ]);
        let mut visits = Vec::new();
        let mut evidence = HashSet::new();
        let result = eval_with_evidence(
            &tree,
            &mut evidence,
            &mut |node, fields| {
                let TestTree::Leaf(name, truth) = node else {
                    unreachable!()
                };
                visits.push(*name);
                if *truth == Some(true) {
                    fields.unwrap().insert((*name).to_owned());
                }
                Ok::<_, ()>(*truth)
            },
            &mut |_| true,
            false,
        )
        .unwrap();
        assert_eq!(result, Some(true));
        assert_eq!(visits, ["first", "second", "unavailable"]);
        assert_eq!(evidence, HashSet::from(["first".into(), "second".into()]));
    }

    #[test]
    fn negation_keeps_unknown_unknown() {
        let tree = TestTree::Not(Box::new(TestTree::Leaf("unavailable", None)));
        let result = eval_membership(&tree, &mut |node| {
            let TestTree::Leaf(_, truth) = node else {
                unreachable!()
            };
            Ok::<_, ()>(*truth)
        })
        .unwrap();
        assert_eq!(result, None);
    }
}
