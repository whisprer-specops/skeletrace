// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Graphs as adjacency lists, and the traversals SkeleTrace's topology views lean on.
//!
//! | Function | Answers | Cost |
//! |---|---|---|
//! | [`bfs`], [`shortest_path`] | How few hops from A to B? What's within k hops? | O(V + E) |
//! | [`dijkstra`], [`cheapest_path`] | The cheapest route when each link has its own cost | O((V + E) log V) |
//! | [`dfs_order`] | What can be reached, going deep first? | O(V + E) |
//! | [`topological_sort`] | In what order can these run, if each needs the ones it depends on? | O(V log V + E) |
//! | [`components`] | Which parts are cut off from each other? | O(V + E · α) |
//! | [`weak_points`] | Which single node or link, if lost, splits a group in two? | O(V + E) |
//!
//! Every traversal is iterative, with its own stack or queue, so a million-node chain can't overflow the call
//! stack the way a recursive version would.
//!
//! [`KeyedGraph`] builds a graph from your own node keys (entity IDs, addresses, handles) and maps results back.

use crate::dsu::DisjointSet;
use crate::heap::MinHeap;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::Hash;

/// Why a graph operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A node index past the end of the graph.
    NodeOutOfRange {
        /// The index given.
        node: usize,
        /// How many nodes the graph has.
        nodes: usize,
    },
    /// The operation needs an undirected graph.
    NeedsUndirected,
    /// An edge cost that isn't a finite number of zero or more.
    BadCost,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::NodeOutOfRange { node, nodes } => write!(
                f,
                "node {node} is out of range: the graph has {nodes} nodes"
            ),
            GraphError::NeedsUndirected => write!(f, "this works on undirected graphs only"),
            GraphError::BadCost => write!(f, "edge costs must be finite and not negative"),
        }
    }
}

impl std::error::Error for GraphError {}

/// A graph over the nodes `0..node_count()`, stored as adjacency lists. Parallel edges and self-loops are
/// allowed.
#[derive(Clone, Debug)]
pub struct Graph {
    adj: Vec<Vec<(usize, usize)>>, // (neighbour, edge id)
    edges: usize,
    directed: bool,
}

impl Graph {
    /// An undirected graph with `nodes` nodes and no edges: each edge works both ways.
    pub fn undirected(nodes: usize) -> Self {
        Self {
            adj: vec![Vec::new(); nodes],
            edges: 0,
            directed: false,
        }
    }

    /// A directed graph with `nodes` nodes and no edges: each edge goes one way, from → to.
    pub fn directed(nodes: usize) -> Self {
        Self {
            adj: vec![Vec::new(); nodes],
            edges: 0,
            directed: true,
        }
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.adj.len()
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges
    }

    /// True for a directed graph.
    pub fn is_directed(&self) -> bool {
        self.directed
    }

    /// Adds a node and returns its index.
    pub fn add_node(&mut self) -> usize {
        self.adj.push(Vec::new());
        self.adj.len() - 1
    }

    /// Adds an edge and returns its id (edges are numbered from 0 in the order added).
    pub fn add_edge(&mut self, from: usize, to: usize) -> Result<usize, GraphError> {
        self.check(from)?;
        self.check(to)?;
        Ok(self.push_edge(from, to))
    }

    /// The neighbours of `node`, in the order their edges were added.
    pub fn neighbours(&self, node: usize) -> Result<impl Iterator<Item = usize> + '_, GraphError> {
        self.check(node)?;
        Ok(self.adj[node].iter().map(|&(to, _)| to))
    }

    /// Number of edge ends at `node` (outgoing edges, for a directed graph). A self-loop counts twice in an
    /// undirected graph.
    pub fn degree(&self, node: usize) -> Result<usize, GraphError> {
        self.check(node)?;
        Ok(self.adj[node].len())
    }

    fn push_edge(&mut self, from: usize, to: usize) -> usize {
        let id = self.edges;
        self.adj[from].push((to, id));
        if !self.directed {
            self.adj[to].push((from, id));
        }
        self.edges += 1;
        id
    }

    fn check(&self, node: usize) -> Result<(), GraphError> {
        if node < self.adj.len() {
            Ok(())
        } else {
            Err(GraphError::NodeOutOfRange {
                node,
                nodes: self.adj.len(),
            })
        }
    }
}

/// The result of a breadth-first search: hop counts and a shortest-path tree from one start node.
#[derive(Clone, Debug)]
pub struct Bfs {
    start: usize,
    dist: Vec<Option<usize>>,
    parent: Vec<Option<usize>>,
    order: Vec<usize>,
}

impl Bfs {
    /// The node the search started from.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Fewest hops from the start to `node`, or `None` if it can't be reached (or doesn't exist).
    pub fn distance(&self, node: usize) -> Option<usize> {
        self.dist.get(node).copied().flatten()
    }

    /// Every reachable node, nearest first.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// A shortest path from the start to `target`, both included, or `None` if unreachable.
    pub fn path_to(&self, target: usize) -> Option<Vec<usize>> {
        self.distance(target)?;
        let mut path = vec![target];
        let mut node = target;
        while let Some(previous) = self.parent[node] {
            path.push(previous);
            node = previous;
        }
        path.reverse();
        Some(path)
    }

    /// Every node within `hops` hops of the start, nearest first ("k-hop neighbourhood").
    pub fn within(&self, hops: usize) -> Vec<usize> {
        self.order
            .iter()
            .copied()
            .take_while(|&v| self.dist[v].is_some_and(|d| d <= hops))
            .collect()
    }
}

/// Breadth-first search from `start`: visit everything one hop away, then two hops, and so on, using a queue.
/// The first time a node is reached is by a shortest path in hops. O(V + E).
///
/// **SkeleTrace use:** "how is A connected to B?" in a case graph, and expanding a node's surroundings.
pub fn bfs(graph: &Graph, start: usize) -> Result<Bfs, GraphError> {
    graph.check(start)?;
    let n = graph.node_count();
    let mut dist = vec![None; n];
    let mut parent = vec![None; n];
    let mut order = Vec::new();
    let mut queue = VecDeque::new();
    dist[start] = Some(0);
    queue.push_back(start);
    while let Some(v) = queue.pop_front() {
        order.push(v);
        let next = dist[v].map_or(0, |d| d + 1);
        for &(to, _) in &graph.adj[v] {
            if dist[to].is_none() {
                dist[to] = Some(next);
                parent[to] = Some(v);
                queue.push_back(to);
            }
        }
    }
    Ok(Bfs {
        start,
        dist,
        parent,
        order,
    })
}

/// A path with the fewest hops from `from` to `to`, or `Ok(None)` if there is none.
pub fn shortest_path(
    graph: &Graph,
    from: usize,
    to: usize,
) -> Result<Option<Vec<usize>>, GraphError> {
    graph.check(to)?;
    Ok(bfs(graph, from)?.path_to(to))
}

/// Depth-first search from `start`: go as deep as possible before backing up. Returns nodes in the order a
/// recursive search would first visit them (neighbours in edge order), using an explicit stack. O(V + E).
pub fn dfs_order(graph: &Graph, start: usize) -> Result<Vec<usize>, GraphError> {
    graph.check(start)?;
    let mut seen = vec![false; graph.node_count()];
    let mut order = vec![start];
    seen[start] = true;
    // Each stack entry is a node and how far through its neighbour list we've got.
    let mut stack = vec![(start, 0usize)];
    while let Some(top) = stack.last_mut() {
        let (v, position) = *top;
        if let Some(&(to, _)) = graph.adj[v].get(position) {
            top.1 += 1;
            if !seen[to] {
                seen[to] = true;
                order.push(to);
                stack.push((to, 0));
            }
        } else {
            stack.pop();
        }
    }
    Ok(order)
}

/// Why a topological order doesn't exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopoError {
    /// Topological order needs a directed graph.
    NotDirected,
    /// The graph has a cycle, so no order works. The nodes of one cycle, in edge order: each has an edge to the
    /// next, and the last has an edge back to the first.
    Cycle(Vec<usize>),
}

impl fmt::Display for TopoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TopoError::NotDirected => write!(f, "topological order needs a directed graph"),
            TopoError::Cycle(nodes) => write!(f, "the graph has a cycle through nodes {nodes:?}"),
        }
    }
}

impl std::error::Error for TopoError {}

/// An order of all nodes in which every edge points forwards, by Kahn's algorithm: repeatedly take a node
/// with nothing left pointing at it. Among the valid orders it returns the smallest in dictionary order, so
/// output is deterministic. O(V log V + E).
///
/// **SkeleTrace use:** running collection steps after the steps whose output they need, and catching circular
/// dependencies, which come back as [`TopoError::Cycle`] with the nodes involved.
pub fn topological_sort(graph: &Graph) -> Result<Vec<usize>, TopoError> {
    if !graph.directed {
        return Err(TopoError::NotDirected);
    }
    let n = graph.node_count();
    let mut indegree = vec![0usize; n];
    for list in &graph.adj {
        for &(to, _) in list {
            indegree[to] += 1;
        }
    }
    let mut ready: MinHeap<usize> = (0..n).filter(|&v| indegree[v] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(v) = ready.pop() {
        order.push(v);
        for &(to, _) in &graph.adj[v] {
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }
    if order.len() == n {
        Ok(order)
    } else {
        Err(TopoError::Cycle(find_cycle(graph, &indegree)))
    }
}

/// After Kahn's algorithm stalls, every node left over still has an edge from another leftover node. Walking
/// backwards along such edges must repeat a node, and the repeat closes a cycle.
fn find_cycle(graph: &Graph, indegree: &[usize]) -> Vec<usize> {
    let n = graph.node_count();
    let leftover: Vec<bool> = indegree.iter().map(|&d| d > 0).collect();
    let mut predecessor: Vec<Option<usize>> = vec![None; n];
    for (v, list) in graph.adj.iter().enumerate() {
        if !leftover[v] {
            continue;
        }
        for &(to, _) in list {
            if leftover[to] && predecessor[to].is_none() {
                predecessor[to] = Some(v);
            }
        }
    }
    let Some(start) = (0..n).find(|&v| leftover[v]) else {
        return Vec::new();
    };
    let mut position = vec![usize::MAX; n];
    let mut walk = Vec::new();
    let mut v = start;
    loop {
        if position[v] != usize::MAX {
            let mut cycle = walk[position[v]..].to_vec();
            cycle.reverse(); // the walk went backwards along edges; reverse it to follow them forwards
            return cycle;
        }
        position[v] = walk.len();
        walk.push(v);
        match predecessor[v] {
            Some(p) => v = p,
            None => return Vec::new(), // can't happen: every leftover node has a leftover predecessor
        }
    }
}

/// The connected components, each as an ascending list of nodes, ordered by their smallest node. For a
/// directed graph, edge directions are ignored ("weakly connected" components). Built on union-find.
pub fn components(graph: &Graph) -> Vec<Vec<usize>> {
    let mut sets = DisjointSet::new(graph.node_count());
    for (v, list) in graph.adj.iter().enumerate() {
        for &(to, _) in list {
            sets.union(v, to);
        }
    }
    sets.groups()
}

/// The single points of failure in an undirected graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeakPoints {
    /// Cut nodes (articulation points), ascending: removing one splits its group in two or more.
    pub cut_nodes: Vec<usize>,
    /// Bridges as `(smaller, larger)` node pairs, ascending: removing one splits its group in two.
    pub bridges: Vec<(usize, usize)>,
}

/// Cut nodes and bridges, by Tarjan's low-link method in one depth-first pass, with an explicit stack.
///
/// Number nodes in the order the search reaches them. A node's low-link is the earliest number reachable from
/// its subtree using one edge that leads back up. If nothing below a child can climb back above the parent,
/// the parent holds that child's subtree on by itself: it's a cut node. If nothing can even climb back to the
/// parent, the edge between them is a bridge. Parallel edges are handled: a doubled link is never a bridge.
/// O(V + E).
///
/// **SkeleTrace use:** the "weak points" view. A group held together by one node: remove it and the group falls
/// into two.
pub fn weak_points(graph: &Graph) -> Result<WeakPoints, GraphError> {
    if graph.directed {
        return Err(GraphError::NeedsUndirected);
    }
    const UNSEEN: usize = usize::MAX;
    let n = graph.node_count();
    let mut order_of = vec![UNSEEN; n];
    let mut low = vec![0usize; n];
    let mut is_cut = vec![false; n];
    let mut bridges = Vec::new();
    let mut counter = 0usize;
    // (node, edge used to reach it, position in its neighbour list)
    let mut stack: Vec<(usize, Option<usize>, usize)> = Vec::new();
    for root in 0..n {
        if order_of[root] != UNSEEN {
            continue;
        }
        order_of[root] = counter;
        low[root] = counter;
        counter += 1;
        let mut root_children = 0;
        stack.push((root, None, 0));
        while let Some(top) = stack.last_mut() {
            let (v, arrived_by, position) = *top;
            if let Some(&(to, edge)) = graph.adj[v].get(position) {
                top.2 += 1;
                if Some(edge) == arrived_by {
                    continue; // don't count the edge we came in on as a way back up
                }
                if order_of[to] == UNSEEN {
                    order_of[to] = counter;
                    low[to] = counter;
                    counter += 1;
                    if v == root {
                        root_children += 1;
                    }
                    stack.push((to, Some(edge), 0));
                } else {
                    low[v] = low[v].min(order_of[to]);
                }
            } else {
                stack.pop();
                if let Some(&(parent, _, _)) = stack.last() {
                    low[parent] = low[parent].min(low[v]);
                    if low[v] > order_of[parent] {
                        bridges.push((parent.min(v), parent.max(v)));
                    }
                    if parent != root && low[v] >= order_of[parent] {
                        is_cut[parent] = true;
                    }
                }
            }
        }
        // The root is special: it's a cut node exactly when the search left it more than once.
        if root_children > 1 {
            is_cut[root] = true;
        }
    }
    bridges.sort_unstable();
    Ok(WeakPoints {
        cut_nodes: (0..n).filter(|&v| is_cut[v]).collect(),
        bridges,
    })
}

/// A graph built from your own node keys (entity IDs, IP addresses, handles), remembering which index is which.
#[derive(Clone, Debug)]
pub struct KeyedGraph<K> {
    graph: Graph,
    index: HashMap<K, usize>,
    keys: Vec<K>,
}

impl<K: Eq + Hash + Clone> KeyedGraph<K> {
    /// An empty undirected graph.
    pub fn undirected() -> Self {
        Self {
            graph: Graph::undirected(0),
            index: HashMap::new(),
            keys: Vec::new(),
        }
    }

    /// An empty directed graph.
    pub fn directed() -> Self {
        Self {
            graph: Graph::directed(0),
            index: HashMap::new(),
            keys: Vec::new(),
        }
    }

    /// The index for `key`, adding a node for it the first time it's seen.
    pub fn node(&mut self, key: &K) -> usize {
        if let Some(&i) = self.index.get(key) {
            return i;
        }
        let i = self.graph.add_node();
        self.index.insert(key.clone(), i);
        self.keys.push(key.clone());
        i
    }

    /// Adds an edge between two keys (adding their nodes if needed) and returns the edge id.
    pub fn link(&mut self, from: &K, to: &K) -> usize {
        let (a, b) = (self.node(from), self.node(to));
        self.graph.push_edge(a, b)
    }

    /// The index of `key`, if it has a node.
    pub fn index_of(&self, key: &K) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// The key of node `index`.
    pub fn key(&self, index: usize) -> Option<&K> {
        self.keys.get(index)
    }

    /// The keys of several nodes, skipping indices that don't exist.
    pub fn keys_of(&self, indices: &[usize]) -> Vec<&K> {
        indices.iter().filter_map(|&i| self.keys.get(i)).collect()
    }

    /// The underlying graph, for the functions in this module.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
}

/// A graph whose edges each carry a cost: a distance, a time, or a measure of doubt (see
/// [`crate::osint::strongest_chain`]).
#[derive(Clone, Debug)]
pub struct WeightedGraph {
    graph: Graph,
    costs: Vec<f64>,
}

impl WeightedGraph {
    /// An undirected graph with `nodes` nodes and no edges.
    pub fn undirected(nodes: usize) -> Self {
        Self {
            graph: Graph::undirected(nodes),
            costs: Vec::new(),
        }
    }

    /// A directed graph with `nodes` nodes and no edges.
    pub fn directed(nodes: usize) -> Self {
        Self {
            graph: Graph::directed(nodes),
            costs: Vec::new(),
        }
    }

    /// Adds a node and returns its index.
    pub fn add_node(&mut self) -> usize {
        self.graph.add_node()
    }

    /// Adds an edge with its cost and returns its id, numbered from 0 in the order added. The cost must be
    /// finite and not negative: Dijkstra's method relies on a route never getting cheaper as it grows.
    pub fn add_edge(&mut self, from: usize, to: usize, cost: f64) -> Result<usize, GraphError> {
        if !(cost.is_finite() && cost >= 0.0) {
            return Err(GraphError::BadCost);
        }
        let id = self.graph.add_edge(from, to)?;
        self.costs.push(cost);
        Ok(id)
    }

    /// The cost of edge `edge`.
    pub fn cost(&self, edge: usize) -> Option<f64> {
        self.costs.get(edge).copied()
    }

    /// The graph without its costs, for the unweighted functions in this module.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
}

/// A node waiting in Dijkstra's queue, cheapest first, then lowest index.
#[derive(Clone, Copy, Debug)]
struct Queued {
    cost: f64,
    node: usize,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cost
            .total_cmp(&other.cost)
            .then(self.node.cmp(&other.node))
    }
}

/// The result of [`dijkstra`]: the cheapest cost from one start node to every node it can reach, and the routes.
#[derive(Clone, Debug)]
pub struct Dijkstra {
    start: usize,
    cost: Vec<Option<f64>>,
    parent: Vec<Option<(usize, usize)>>, // (previous node, edge used)
    order: Vec<usize>,
}

impl Dijkstra {
    /// The node the search started from.
    pub fn start(&self) -> usize {
        self.start
    }

    /// The cheapest total cost from the start to `node`, or `None` if it can't be reached (or doesn't exist).
    pub fn cost_to(&self, node: usize) -> Option<f64> {
        self.cost.get(node).copied().flatten()
    }

    /// Every reachable node, cheapest first: the order the search settled them in.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// A cheapest route from the start to `target`, both included, or `None` if unreachable.
    pub fn path_to(&self, target: usize) -> Option<Vec<usize>> {
        self.cost_to(target)?;
        let mut path = vec![target];
        let mut node = target;
        while let Some((previous, _)) = self.parent[node] {
            path.push(previous);
            node = previous;
        }
        path.reverse();
        Some(path)
    }

    /// The edge ids along [`Dijkstra::path_to`], in order, or `None` if unreachable. Where two nodes are joined
    /// by several edges, this says which one the route used.
    pub fn edges_to(&self, target: usize) -> Option<Vec<usize>> {
        self.cost_to(target)?;
        let mut edges = Vec::new();
        let mut node = target;
        while let Some((previous, edge)) = self.parent[node] {
            edges.push(edge);
            node = previous;
        }
        edges.reverse();
        Some(edges)
    }
}

/// Cheapest routes from `start` to every node, by Dijkstra's method: settle nodes in order of cost, cheapest
/// first. Once the cheapest unsettled node comes off the queue, no other route can reach it for less, because
/// costs are never negative, so its cost is final. O((V + E) log V), with [`MinHeap`] as the queue.
///
/// On ties the route found first is kept. Costs add up as `f64`.
pub fn dijkstra(graph: &WeightedGraph, start: usize) -> Result<Dijkstra, GraphError> {
    graph.graph.check(start)?;
    let n = graph.graph.node_count();
    let mut cost: Vec<Option<f64>> = vec![None; n];
    let mut parent = vec![None; n];
    let mut settled = vec![false; n];
    let mut order = Vec::new();
    let mut queue = MinHeap::new();
    cost[start] = Some(0.0);
    queue.push(Queued {
        cost: 0.0,
        node: start,
    });
    while let Some(Queued { cost: so_far, node }) = queue.pop() {
        if settled[node] {
            continue; // a stale queue entry: this node was settled more cheaply already
        }
        settled[node] = true;
        order.push(node);
        for &(to, edge) in &graph.graph.adj[node] {
            if settled[to] {
                continue;
            }
            let through = so_far + graph.costs[edge];
            if cost[to].map_or(true, |known| through < known) {
                cost[to] = Some(through);
                parent[to] = Some((node, edge));
                queue.push(Queued {
                    cost: through,
                    node: to,
                });
            }
        }
    }
    Ok(Dijkstra {
        start,
        cost,
        parent,
        order,
    })
}

/// A cheapest route from `from` to `to` and its total cost, or `Ok(None)` if there is none.
pub fn cheapest_path(
    graph: &WeightedGraph,
    from: usize,
    to: usize,
) -> Result<Option<(f64, Vec<usize>)>, GraphError> {
    graph.graph.check(to)?;
    let result = dijkstra(graph, from)?;
    Ok(result.cost_to(to).zip(result.path_to(to)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    fn undirected(n: usize, edges: &[(usize, usize)]) -> Graph {
        let mut g = Graph::undirected(n);
        for &(a, b) in edges {
            g.add_edge(a, b).unwrap();
        }
        g
    }

    #[test]
    fn bfs_hops_paths_and_neighbourhoods() {
        //  0 - 1 - 2 - 3      4 (alone)
        //   \_____/
        let g = undirected(5, &[(0, 1), (1, 2), (2, 3), (0, 2)]);
        let r = bfs(&g, 0).unwrap();
        assert_eq!(r.distance(3), Some(2));
        assert_eq!(r.distance(4), None);
        assert_eq!(r.path_to(3), Some(vec![0, 2, 3]));
        assert_eq!(r.path_to(0), Some(vec![0]));
        assert_eq!(r.within(1), vec![0, 1, 2]);
        assert_eq!(shortest_path(&g, 3, 1).unwrap(), Some(vec![3, 2, 1]));
        assert_eq!(shortest_path(&g, 0, 4).unwrap(), None);
        assert!(matches!(
            bfs(&g, 9),
            Err(GraphError::NodeOutOfRange { node: 9, nodes: 5 })
        ));
    }

    #[test]
    fn dfs_goes_deep_first() {
        let g = undirected(6, &[(0, 1), (0, 2), (1, 3), (1, 4), (2, 5)]);
        assert_eq!(dfs_order(&g, 0).unwrap(), vec![0, 1, 3, 4, 2, 5]);
        let mut d = Graph::directed(3);
        d.add_edge(1, 0).unwrap();
        d.add_edge(1, 2).unwrap();
        assert_eq!(dfs_order(&d, 0).unwrap(), vec![0], "edges only go one way");
    }

    #[test]
    fn topological_order_and_cycles() {
        let mut g = Graph::directed(6);
        for (a, b) in [(5, 2), (5, 0), (4, 0), (4, 1), (2, 3), (3, 1)] {
            g.add_edge(a, b).unwrap();
        }
        assert_eq!(topological_sort(&g).unwrap(), vec![4, 5, 0, 2, 3, 1]);
        g.add_edge(1, 2).unwrap(); // 2 -> 3 -> 1 -> 2
        match topological_sort(&g) {
            Err(TopoError::Cycle(cycle)) => {
                let mut members = cycle.clone();
                members.sort_unstable();
                assert_eq!(members, vec![1, 2, 3]);
                for i in 0..cycle.len() {
                    let (a, b) = (cycle[i], cycle[(i + 1) % cycle.len()]);
                    assert!(
                        g.neighbours(a).unwrap().any(|x| x == b),
                        "{a} -> {b} should be an edge"
                    );
                }
            }
            other => panic!("expected a cycle, got {other:?}"),
        }
        assert_eq!(
            topological_sort(&undirected(2, &[(0, 1)])),
            Err(TopoError::NotDirected)
        );
        let mut self_loop = Graph::directed(2);
        self_loop.add_edge(1, 1).unwrap();
        assert_eq!(topological_sort(&self_loop), Err(TopoError::Cycle(vec![1])));
    }

    #[test]
    fn components_split_correctly() {
        let g = undirected(6, &[(0, 1), (2, 3), (3, 4)]);
        assert_eq!(components(&g), vec![vec![0, 1], vec![2, 3, 4], vec![5]]);
    }

    #[test]
    fn weak_points_on_known_shapes() {
        let path = undirected(3, &[(0, 1), (1, 2)]);
        assert_eq!(
            weak_points(&path).unwrap(),
            WeakPoints {
                cut_nodes: vec![1],
                bridges: vec![(0, 1), (1, 2)]
            }
        );
        let triangle = undirected(3, &[(0, 1), (1, 2), (2, 0)]);
        assert_eq!(weak_points(&triangle).unwrap(), WeakPoints::default());
        let bowtie = undirected(5, &[(0, 1), (1, 2), (2, 0), (2, 3), (3, 4), (4, 2)]);
        assert_eq!(
            weak_points(&bowtie).unwrap(),
            WeakPoints {
                cut_nodes: vec![2],
                bridges: vec![]
            }
        );
        let doubled = undirected(3, &[(0, 1), (0, 1), (1, 2)]);
        assert_eq!(
            weak_points(&doubled).unwrap(),
            WeakPoints {
                cut_nodes: vec![1],
                bridges: vec![(1, 2)]
            }
        );
        let mut directed = Graph::directed(2);
        directed.add_edge(0, 1).unwrap();
        assert_eq!(weak_points(&directed), Err(GraphError::NeedsUndirected));
    }

    fn component_count(
        n: usize,
        edges: &[(usize, usize)],
        skip_node: Option<usize>,
        skip_edge: Option<usize>,
    ) -> usize {
        let mut d = DisjointSet::new(n);
        for (id, &(a, b)) in edges.iter().enumerate() {
            if Some(id) == skip_edge || Some(a) == skip_node || Some(b) == skip_node {
                continue;
            }
            d.union(a, b);
        }
        d.set_count() - usize::from(skip_node.is_some())
    }

    #[test]
    fn weak_points_match_brute_force() {
        let mut rng = Lcg(81);
        for _ in 0..400 {
            let n = 1 + rng.below(8) as usize;
            let m = rng.below(12) as usize;
            let edges: Vec<(usize, usize)> = (0..m)
                .map(|_| (rng.below(n as u64) as usize, rng.below(n as u64) as usize))
                .collect();
            let g = undirected(n, &edges);
            let found = weak_points(&g).unwrap();
            let base = component_count(n, &edges, None, None);
            let cut: Vec<usize> = (0..n)
                .filter(|&v| component_count(n, &edges, Some(v), None) > base)
                .collect();
            let mut bridges: Vec<(usize, usize)> = (0..m)
                .filter(|&e| component_count(n, &edges, None, Some(e)) > base)
                .map(|e| (edges[e].0.min(edges[e].1), edges[e].0.max(edges[e].1)))
                .collect();
            bridges.sort_unstable();
            assert_eq!(found.cut_nodes, cut, "edges {edges:?}");
            assert_eq!(found.bridges, bridges, "edges {edges:?}");
        }
    }

    #[test]
    fn deep_graphs_do_not_overflow_the_stack() {
        let n = 300_000;
        let mut g = Graph::undirected(n);
        for i in 1..n {
            g.add_edge(i - 1, i).unwrap();
        }
        assert_eq!(dfs_order(&g, 0).unwrap().len(), n);
        let w = weak_points(&g).unwrap();
        assert_eq!((w.cut_nodes.len(), w.bridges.len()), (n - 2, n - 1));
    }

    #[test]
    fn keyed_graph_maps_back_to_keys() {
        let mut kg = KeyedGraph::undirected();
        kg.link(&"alice@example.org", &"@alice");
        kg.link(&"@alice", &"+44 7700 900123");
        kg.link(&"+44 7700 900123", &"bob@example.org");
        let (from, to) = (
            kg.index_of(&"alice@example.org").unwrap(),
            kg.index_of(&"bob@example.org").unwrap(),
        );
        let path = shortest_path(kg.graph(), from, to).unwrap().unwrap();
        assert_eq!(
            kg.keys_of(&path),
            vec![
                &"alice@example.org",
                &"@alice",
                &"+44 7700 900123",
                &"bob@example.org"
            ]
        );
        assert_eq!(kg.key(99), None);
        assert_eq!(kg.graph().edge_count(), 3);
    }

    #[test]
    fn dijkstra_prefers_cheap_detours() {
        //  0 --9-- 1          the direct link costs 9,
        //   \     /           the detour through 2 costs 2 + 3
        //    2   3
        //     \ /
        //      2
        let mut g = WeightedGraph::undirected(4);
        g.add_edge(0, 1, 9.0).unwrap();
        g.add_edge(0, 2, 2.0).unwrap();
        g.add_edge(2, 1, 3.0).unwrap();
        let r = dijkstra(&g, 0).unwrap();
        assert_eq!(r.cost_to(1), Some(5.0));
        assert_eq!(r.path_to(1), Some(vec![0, 2, 1]));
        assert_eq!(r.edges_to(1), Some(vec![1, 2]));
        assert_eq!(r.cost_to(3), None, "node 3 has no links");
        assert_eq!(r.order(), &[0, 2, 1]);
        assert_eq!(cheapest_path(&g, 1, 0).unwrap(), Some((5.0, vec![1, 2, 0])));
        assert_eq!(cheapest_path(&g, 0, 3).unwrap(), None);
        assert_eq!(cheapest_path(&g, 0, 0).unwrap(), Some((0.0, vec![0])));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(g.add_edge(0, 1, bad), Err(GraphError::BadCost));
        }
        assert!(matches!(
            g.add_edge(0, 7, 1.0),
            Err(GraphError::NodeOutOfRange { node: 7, nodes: 4 })
        ));
        assert_eq!(g.cost(0), Some(9.0));
        assert_eq!(g.graph().edge_count(), 3, "refused edges aren't added");
    }

    #[test]
    fn dijkstra_matches_bellman_ford() {
        let mut rng = Lcg(97);
        for round in 0..400 {
            let n = 1 + rng.below(9) as usize;
            let directed = round % 2 == 0;
            let mut g = if directed {
                WeightedGraph::directed(n)
            } else {
                WeightedGraph::undirected(n)
            };
            let mut edges = Vec::new();
            for _ in 0..rng.below(20) {
                let (a, b) = (rng.below(n as u64) as usize, rng.below(n as u64) as usize);
                let cost = rng.below(10) as f64; // whole numbers add up exactly
                g.add_edge(a, b, cost).unwrap();
                edges.push((a, b, cost));
            }
            let start = rng.below(n as u64) as usize;
            // Bellman-Ford: relax every edge n times.
            let mut best: Vec<Option<f64>> = vec![None; n];
            best[start] = Some(0.0);
            for _ in 0..n {
                for &(a, b, c) in &edges {
                    let pairs: &[(usize, usize)] = if directed {
                        &[(a, b)]
                    } else {
                        &[(a, b), (b, a)]
                    };
                    for &(x, y) in pairs {
                        if let Some(d) = best[x] {
                            if best[y].map_or(true, |e| d + c < e) {
                                best[y] = Some(d + c);
                            }
                        }
                    }
                }
            }
            let r = dijkstra(&g, start).unwrap();
            for (target, &want) in best.iter().enumerate() {
                assert_eq!(r.cost_to(target), want, "round {round}");
                if let (Some(path), Some(ids)) = (r.path_to(target), r.edges_to(target)) {
                    assert_eq!(path.first(), Some(&start));
                    assert_eq!(path.last(), Some(&target));
                    assert_eq!(ids.len() + 1, path.len());
                    let mut total = 0.0;
                    for (k, &id) in ids.iter().enumerate() {
                        let (a, b, c) = edges[id];
                        let (x, y) = (path[k], path[k + 1]);
                        assert!((a, b) == (x, y) || (!directed && (b, a) == (x, y)));
                        total += c;
                    }
                    assert_eq!(Some(total), want);
                }
            }
        }
    }
}
