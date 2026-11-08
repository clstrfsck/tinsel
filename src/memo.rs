//! Automatic memoization decision (SPEC 8.1).
//!
//! A function is memoized when it is in a recursive strongly connected
//! component of the static call graph and at least one of its references to
//! a function in that component is not a tail call. `@memo` and `@nomemo`
//! override the decision.

use crate::ast::Annotation;

/// `edges` are (caller, callee, is_tail_call). Returns a flag per function.
pub fn decide(annotations: &[Option<Annotation>], edges: &[(usize, usize, bool)]) -> Vec<bool> {
    let n = annotations.len();
    let mut adj = vec![Vec::new(); n];
    for &(a, b, _) in edges {
        adj[a].push(b);
    }
    let comp = tarjan(&adj);

    let mut memo = vec![false; n];
    for &(a, b, tail) in edges {
        if comp[a] == comp[b] && !tail {
            memo[a] = true;
        }
    }
    for (i, ann) in annotations.iter().enumerate() {
        match ann {
            Some(Annotation::Memo) => memo[i] = true,
            Some(Annotation::NoMemo) => memo[i] = false,
            None => {}
        }
    }
    memo
}

/// Tarjan's algorithm, iterative so deep call graphs can't overflow.
/// Returns the component index of each node. A node with no self-edge that
/// is alone in its component isn't recursive; `decide` only looks at edges,
/// so such nodes never get marked.
fn tarjan(adj: &[Vec<usize>]) -> Vec<usize> {
    let n = adj.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut comp = vec![usize::MAX; n];
    let mut next_index = 0;
    let mut next_comp = 0;

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, next edge to visit)
        let mut work = vec![(root, 0usize)];
        while let Some(&(v, ei)) = work.last() {
            if ei == 0 && index[v] == usize::MAX {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if ei < adj[v].len() {
                let w = adj[v][ei];
                if let Some(top) = work.last_mut() {
                    top.1 += 1;
                }
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp[w] = next_comp;
                    if w == v {
                        break;
                    }
                }
                next_comp += 1;
            }
        }
    }
    comp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_functions() {
        // 0: fib (non-tail self call), 1/2: mutual tail recursion,
        // 3: calls fib but isn't recursive, 4: loop.
        let edges = [
            (0, 0, false),
            (0, 0, false),
            (1, 2, true),
            (2, 1, true),
            (3, 0, true),
            (4, 4, true),
        ];
        let memo = decide(&[None; 5], &edges);
        assert_eq!(memo, vec![true, false, false, false, false]);
    }

    #[test]
    fn annotations_override() {
        let memo = decide(
            &[Some(Annotation::NoMemo), Some(Annotation::Memo)],
            &[(0, 0, false)],
        );
        assert_eq!(memo, vec![false, true]);
    }
}
