//! Phase 3 Barnes-Hut quadtree for body-particle gravity.
//!
//! See `docs/phase-3-barnes-hut.md` for the algorithm.
//!
//! The tree is a flat `Vec<QuadNode>` (no `Box`), so memory is contiguous
//! and cache-friendly. For N bodies the tree has at most 4N nodes. The
//! tree is rebuilt every tick — bodies move, so incremental updates
//! would be more bookkeeping than they're worth at this scale.
//!
//! The traversal computes per-particle acceleration with the
//! `s/d < theta` opening-angle criterion (Plummer-softened).
//! theta=0.5 is the spec default; lower is more accurate, higher is
//! faster.

use crate::body::{Body, G, SOFTENING_SQ};
use crate::body::Vec2;

/// Axis-aligned rectangle. QuadTree bounds, in the same coordinate
/// space as `Body::position` (grid cells, sub-cell f32).
#[derive(Copy, Clone, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Bounds of the i-th quadrant of `self`. Quadrant order is
    /// NW, NE, SW, SE = 0, 1, 2, 3. A point at exactly the mid-line
    /// is assigned to the right/bottom quadrant (matches
    /// `point_quadrant`).
    pub fn quadrant_bounds(&self, i: usize) -> Rect {
        let hw = self.w * 0.5;
        let hh = self.h * 0.5;
        match i {
            0 => Rect { x: self.x,         y: self.y,         w: hw, h: hh }, // NW
            1 => Rect { x: self.x + hw,    y: self.y,         w: hw, h: hh }, // NE
            2 => Rect { x: self.x,         y: self.y + hh,    w: hw, h: hh }, // SW
            3 => Rect { x: self.x + hw,    y: self.y + hh,    w: hw, h: hh }, // SE
            _ => unreachable!("quadrant index out of range: {i}"),
        }
    }

    /// Which quadrant of `self` does `p` fall into? Same ordering as
    /// `quadrant_bounds`. A point exactly on the mid-line is
    /// assigned right/bottom.
    #[inline]
    pub fn point_quadrant(&self, p: Vec2) -> usize {
        let mid_x = self.x + self.w * 0.5;
        let mid_y = self.y + self.h * 0.5;
        if p.x < mid_x {
            if p.y < mid_y { 0 } else { 2 }
        } else {
            if p.y < mid_y { 1 } else { 3 }
        }
    }
}

/// One quadtree node. A node is either:
/// * a leaf holding a single body (`body_id = Some(id)`,
///   `children = None`); or
/// * a leaf holding multiple bodies that landed in the same cell
///   (`body_id = None`, `children = None`); or
/// * an internal node (`body_id = None`, `children = Some([..; 4])`).
///
/// `center_of_mass` and `total_mass` are always the mass-weighted CoM
/// and total mass of everything in this node's subtree. For
/// `compute_accel` we treat all three cases uniformly except for the
/// `body_id.is_some()` short-circuit (a single body has zero extent
/// to its own CoM, so the `s/d < theta` test is meaningless).
pub struct QuadNode {
    pub center_of_mass: Vec2,
    pub total_mass: f32,
    pub bounds: Rect,
    /// `Some(id)` iff this leaf holds exactly one body.
    pub body_id: Option<u32>,
    /// `Some([c0, c1, c2, c3])` iff this is an internal node. Indices
    /// into the parent `QuadTree::nodes` vec.
    pub children: Option<[usize; 4]>,
}

/// Flat quadtree. Root is `nodes[root]`. For N bodies the maximum
/// tree size is 4N nodes (one root + up to 3 more layers of 4x
/// expansion), well within L2 cache for any reasonable N.
pub struct QuadTree {
    pub nodes: Vec<QuadNode>,
    pub root: usize,
    /// Barnes-Hut opening-angle parameter. 0.5 is the spec default.
    pub theta: f32,
}

/// Maximum subdivision depth. For a degenerate input where many
/// bodies land in the same cell we'd otherwise subdivide forever; at
/// this depth we treat the leaf as a multi-body aggregate (no more
/// children). 64 is far more than enough for any reasonable N.
const MAX_DEPTH: usize = 64;

impl QuadTree {
    /// Build a quadtree from a slice of bodies. `theta` is the
    /// Barnes-Hut opening-angle parameter. Empty input yields an
    /// empty tree (root points to no nodes); traversal on an empty
    /// tree returns zero acceleration.
    pub fn new(bodies: &[Body], theta: f32) -> Self {
        if bodies.is_empty() {
            return Self {
                nodes: Vec::new(),
                root: 0,
                theta,
            };
        }

        // Tight bounding box of all body positions, with a 1-cell
        // margin on every side so body positions never sit exactly
        // on a quadrant boundary (which would make point_quadrant
        // ambiguous at the corners).
        let (min, max) = bodies.iter().fold(
            (Vec2::new(f32::MAX, f32::MAX), Vec2::new(f32::MIN, f32::MIN)),
            |(lo, hi), b| (lo.min(b.position), hi.max(b.position)),
        );
        let margin = 1.0_f32;
        let bounds = Rect {
            x: min.x - margin,
            y: min.y - margin,
            w: (max.x - min.x).max(1.0) + 2.0 * margin,
            h: (max.y - min.y).max(1.0) + 2.0 * margin,
        };

        // Pre-allocate. Max tree size is 4N; in practice we usually
        // see well under that. Slight over-allocation is fine.
        let mut nodes: Vec<QuadNode> = Vec::with_capacity(4 * bodies.len() + 1);
        let root = nodes.len();
        nodes.push(QuadNode {
            center_of_mass: Vec2::ZERO,
            total_mass: 0.0,
            bounds,
            body_id: None,
            children: None,
        });

        let mut tree = Self { nodes, root, theta };

        for body in bodies {
            tree.insert(body, 0);
        }
        tree.compute_centers_bottom_up(tree.root);
        tree
    }

    /// Insert a body into the tree, subdividing leaves as needed.
    /// `depth` is the current depth in the tree (root is 0) — once
    /// we hit `MAX_DEPTH` we stop subdividing and merge the new body
    /// into the existing leaf.
    fn insert(&mut self, body: &Body, depth: usize) {
        let mut node_idx = self.root;
        let mut current_depth = depth;
        loop {
            // Read-only check of the current node.
            let action = {
                let node = &self.nodes[node_idx];
                if node.body_id.is_none() && node.children.is_none() {
                    Action::PlaceHere
                } else if node.body_id.is_some() {
                    if current_depth >= MAX_DEPTH {
                        Action::MergeHere
                    } else {
                        Action::Subdivide
                    }
                } else {
                    // Internal node: descend.
                    Action::Descend(node.bounds.point_quadrant(body.position))
                }
            };
            match action {
                Action::PlaceHere => {
                    let node = &mut self.nodes[node_idx];
                    node.body_id = Some(body.id);
                    node.center_of_mass = body.position;
                    node.total_mass = body.mass;
                    return;
                }
                Action::MergeHere => {
                    // Treat the leaf as a multi-body aggregate: the
                    // new body shares this node with the existing
                    // one. Drop the `body_id` so the tree
                    // distinguishes "single body" leaves from
                    // "multi body" leaves — the s/d test will then
                    // apply at the right granularity.
                    let node = &mut self.nodes[node_idx];
                    let new_total = node.total_mass + body.mass;
                    node.center_of_mass = (node.center_of_mass * node.total_mass
                        + body.position * body.mass)
                        / new_total;
                    node.total_mass = new_total;
                    node.body_id = None; // multi-body leaf now
                    return;
                }
                Action::Subdivide => {
                    self.subdivide(node_idx);
                    current_depth += 1;
                    // Re-evaluate the (now-internal) node.
                    continue;
                }
                Action::Descend(q) => {
                    let child_idx = self.nodes[node_idx].children.unwrap()[q];
                    node_idx = child_idx;
                }
            }
        }
    }

    /// Split `node_idx` into 4 children and re-insert the existing
    /// body into the appropriate child. The other three children
    /// are empty leaves.
    fn subdivide(&mut self, node_idx: usize) {
        let existing_body_id = self.nodes[node_idx].body_id.unwrap();
        let existing_pos = self.nodes[node_idx].center_of_mass;
        let existing_mass = self.nodes[node_idx].total_mass;
        let bounds = self.nodes[node_idx].bounds;

        // Clear the parent leaf.
        let parent = &mut self.nodes[node_idx];
        parent.body_id = None;
        parent.center_of_mass = Vec2::ZERO;
        parent.total_mass = 0.0;

        // Create 4 children.
        let mut children = [0usize; 4];
        for i in 0..4 {
            let child_idx = self.nodes.len();
            children[i] = child_idx;
            self.nodes.push(QuadNode {
                center_of_mass: Vec2::ZERO,
                total_mass: 0.0,
                bounds: bounds.quadrant_bounds(i),
                body_id: None,
                children: None,
            });
        }
        self.nodes[node_idx].children = Some(children);

        // Place the existing body in its child.
        let q = bounds.point_quadrant(existing_pos);
        let existing_child = children[q];
        let child = &mut self.nodes[existing_child];
        child.body_id = Some(existing_body_id);
        child.center_of_mass = existing_pos;
        child.total_mass = existing_mass;
    }

    /// Recompute each internal node's `center_of_mass` and
    /// `total_mass` as the mass-weighted average of its children.
    /// Run once after all bodies are inserted.
    fn compute_centers_bottom_up(&mut self, node_idx: usize) {
        let children = self.nodes[node_idx].children;
        if let Some(children) = children {
            for &c in &children {
                self.compute_centers_bottom_up(c);
            }
            let mut total_mass = 0.0_f32;
            let mut com = Vec2::ZERO;
            for &c in &children {
                let child = &self.nodes[c];
                com += child.center_of_mass * child.total_mass;
                total_mass += child.total_mass;
            }
            if total_mass > 0.0 {
                com /= total_mass;
            }
            let node = &mut self.nodes[node_idx];
            node.center_of_mass = com;
            node.total_mass = total_mass;
        }
    }

    /// Net gravitational acceleration at `particle_pos` (a cell
    /// position) from all bodies in the tree, Plummer-softened.
    /// `theta` is the opening-angle criterion (smaller = more
    /// accurate, larger = faster).
    pub fn compute_accel(&self, particle_pos: Vec2) -> Vec2 {
        if self.nodes.is_empty() {
            return Vec2::ZERO;
        }
        let theta_sq = self.theta * self.theta;
        self.compute_accel_node(particle_pos, self.root, theta_sq)
    }

    fn compute_accel_node(
        &self,
        p: Vec2,
        node_idx: usize,
        theta_sq: f32,
    ) -> Vec2 {
        let node = &self.nodes[node_idx];
        let r = node.center_of_mass - p;
        let d2_phys = r.length_squared();
        let d2 = d2_phys + SOFTENING_SQ;
        let d = d2.sqrt();
        // Use max(w, h) for the "extent" of the node. For a square
        // cell (typical case) this equals w = h.
        let s = node.bounds.w.max(node.bounds.h);

        // Single-body leaf: always treat as a point mass. (A single
        // body's `s/d` would be infinity because d=0, so the s/d
        // test would incorrectly say "not far enough".)
        if node.body_id.is_some() {
            let a_mag = G * node.total_mass / d2;
            return r * (a_mag / d);
        }

        // s/d < theta is equivalent to s^2 / d^2 < theta^2 once we
        // substitute d^2 for d^2. (The d^2 in d2 includes
        // SOFTENING_SQ, but that's still a valid d^2 for this
        // comparison — softening only matters in the magnitude.)
        // We use the unsoftened d for the test so that the test
        // matches the textbook Barnes-Hut criterion; the softened
        // distance goes into the magnitude.
        if s * s < theta_sq * d2_phys.max(1e-12) {
            let a_mag = G * node.total_mass / d2;
            return r * (a_mag / d);
        }

        // Otherwise recurse into children.
        if let Some(children) = node.children {
            let mut total = Vec2::ZERO;
            for &c in &children {
                total += self.compute_accel_node(p, c, theta_sq);
            }
            return total;
        }
        // Multi-body leaf that failed the s/d test: also treat as a
        // point mass (rare path; only triggers at MAX_DEPTH).
        let a_mag = G * node.total_mass / d2;
        r * (a_mag / d)
    }
}

/// Internal action enum for the `insert` state machine.
enum Action {
    PlaceHere,
    MergeHere,
    Subdivide,
    Descend(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bh() -> Body {
        Body::new_black_hole(1, 100.0, 100.0, 1000.0)
    }

    #[test]
    fn empty_bodies_yields_empty_tree() {
        let t = QuadTree::new(&[], 0.5);
        assert!(t.nodes.is_empty());
        let a = t.compute_accel(Vec2::new(50.0, 50.0));
        assert_eq!(a, Vec2::ZERO);
    }

    #[test]
    fn single_body_tree_has_one_node() {
        let t = QuadTree::new(&[bh()], 0.5);
        assert_eq!(t.nodes.len(), 1);
        let a = t.compute_accel(Vec2::new(0.0, 0.0));
        // Pull toward (100, 100) from (0, 0): positive x and y.
        assert!(a.x > 0.0);
        assert!(a.y > 0.0);
    }

    #[test]
    fn two_distant_bodies_subdivide_root() {
        let b1 = Body::new_black_hole(1, 10.0, 10.0, 100.0);
        let b2 = Body::new_black_hole(2, 200.0, 200.0, 100.0);
        let t = QuadTree::new(&[b1, b2], 0.5);
        // Root is internal (2 bodies in different quadrants).
        let root = &t.nodes[t.root];
        assert!(root.children.is_some());
        assert_eq!(root.total_mass, 200.0);
        // CoM is the midpoint.
        assert!((root.center_of_mass.x - 105.0).abs() < 1.0);
        assert!((root.center_of_mass.y - 105.0).abs() < 1.0);
    }

    #[test]
    fn bh_matches_direct_sum_with_small_theta() {
        // With a very small theta, Barnes-Hut should essentially
        // walk the whole tree and match a direct N^2 sum. We check
        // that the per-cell accel vector from the tree is close to
        // the direct sum for a small set of cells.
        let bodies = vec![
            Body::new_black_hole(1, 50.0, 128.0, 1000.0),
            Body::new_black_hole(2, 200.0, 128.0, 500.0),
            Body::new_planet(3, 128.0, 50.0, 0.0, 0.0, 10),
        ];
        let tree = QuadTree::new(&bodies, 0.1); // small theta = essentially exact

        // Direct sum for comparison.
        let direct = |p: Vec2| -> Vec2 {
            let mut a = Vec2::ZERO;
            for b in &bodies {
                let r = b.position - p;
                let d2 = r.length_squared() + SOFTENING_SQ;
                let d = d2.sqrt();
                let a_mag = G * b.mass / d2;
                a += r * (a_mag / d);
            }
            a
        };

        for &(x, y) in &[(10, 10), (128, 128), (240, 240), (50, 200)] {
            let p = Vec2::new(x as f32, y as f32);
            let a_tree = tree.compute_accel(p);
            let a_direct = direct(p);
            let diff = (a_tree - a_direct).length();
            let mag = a_direct.length();
            // Relative error must be small. With theta=0.1 on a
            // 3-body tree we get exact answers.
            assert!(
                diff < 1e-3 * (mag + 1.0),
                "Barnes-Hut diverges from direct sum at ({x},{y}): tree={a_tree:?}, direct={a_direct:?}"
            );
        }
    }

    #[test]
    fn bh_at_root_treats_as_point_mass_with_high_theta() {
        // With theta=100 (huge), the root is always "far enough"
        // and the entire tree collapses to a single point mass at
        // the system CoM. Verify that.
        let bodies = vec![
            Body::new_black_hole(1, 10.0, 10.0, 100.0),
            Body::new_black_hole(2, 200.0, 200.0, 300.0),
        ];
        let tree = QuadTree::new(&bodies, 100.0);
        // CoM is (10*100 + 200*300) / 400, (10*100 + 200*300)/400.
        let com_x = (10.0 * 100.0 + 200.0 * 300.0) / 400.0;
        let com_y = com_x;
        let p = Vec2::new(0.0, 0.0);
        let a = tree.compute_accel(p);
        let r = Vec2::new(com_x, com_y);
        let d2 = r.length_squared() + SOFTENING_SQ;
        let d = d2.sqrt();
        let a_mag = G * 400.0 / d2;
        let expected = r * (a_mag / d);
        assert!((a - expected).length() < 1e-3);
    }
}
