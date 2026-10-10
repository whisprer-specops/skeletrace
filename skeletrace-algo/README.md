# skeletrace-algo

Graph, ordering, search and range-query building blocks for SkeleTrace, plus the OSINT helpers built on them.
Rust, standard library only: no dependencies, no `unsafe`, nothing to audit in the supply chain.

The algorithms are textbook ideas; the code is original. Every module's documentation says what it does,
how it works and what it costs (`cargo doc --open`).

## Run it

Windows PowerShell (Linux: the same commands):

```
cd skeletrace-algo
cargo test                                      # 92 tests
cargo clippy --all-targets -- -D warnings
cargo run --example tour                        # every module run on one made-up case
cargo run --example tour -- geo                 # one section; `-- list` names them all
cargo run --release --example growth            # watch O(N) and O(log N) pull apart
cargo doc --open                                # the docs, with complexity tables
```

Checked with Rust 1.97. The 8 October version was also checked on 1.91 and on 1.75, the declared minimum
(`rust-version` in `Cargo.toml`). Everything added since passed clippy's check for anything newer than 1.75,
but hasn't been compiled on 1.75 itself. The tour example has only been run on 1.97.

## Add it to SkeleTrace

It's already in. The engine repository keeps this folder at its root, and the engine's `Cargo.toml` names it as
a plain path dependency, with no workspace:

```toml
skeletrace-algo = { path = "skeletrace-algo" }
```

To use it from another crate, add the same line with the path from that crate's folder to this one.

Your own identifiers plug straight in through `KeyedGraph` and `EntityClusters`, which take any key that is
`Eq + Hash + Clone`:

```rust
use skeletrace_algo::graph::{shortest_path, weak_points, KeyedGraph};

let mut g = KeyedGraph::undirected();
g.link(&"198.51.100.7", &"AS64500");
g.link(&"AS64500", &"AS64501");
let weak = weak_points(g.graph())?;          // cut nodes and bridges
let single_points_of_failure = g.keys_of(&weak.cut_nodes);
```

Sweeping a dump file for a watch list, a megabyte at a time, so the file never has to fit in memory:

```rust
use skeletrace_algo::textsearch::PatternSet;
use std::io::Read;

let watch = ["lantern@example.org", "@moth_relay", "+44 7700 900123"];
let set = PatternSet::new_ascii_case_insensitive(watch)?;
let mut scanner = set.scanner();
let mut file = std::fs::File::open("dump.txt")?;
let mut buffer = vec![0u8; 1 << 20];
let mut hits = Vec::new();
loop {
    let read = file.read(&mut buffer)?;
    if read == 0 {
        break;
    }
    scanner.feed(&buffer[..read], &mut hits);
    for hit in hits.drain(..) {
        println!("{} at byte {}", watch[hit.pattern], hit.start);
    }
}
```

## Where it helps SkeleTrace

| SkeleTrace job | Use |
|---|---|
| How is A connected to B (case graph) | `graph::KeyedGraph` + `graph::shortest_path` |
| What's within k hops of this node | `graph::bfs(..).within(k)` |
| Weak points: cut nodes and bridges | `graph::weak_points` |
| Separate clusters in a topology | `graph::components` |
| Collection steps that depend on each other | `graph::topological_sort` (cycles come back with their nodes) |
| Which identifiers belong to one actor | `osint::EntityClusters` |
| Internet nodes layer: IP to ASN or country | `search::Ipv4RangeTable`, `search::cidr_bounds` |
| ADS-B and AIS: combining receivers into one timeline | `merge::merge_sorted` |
| Messages arriving a few places late | `sort::sort_bounded_offset` |
| The most believable chain of evidence between two identifiers | `osint::strongest_chain`, on `graph::dijkstra` |
| Handles and names that differ by typos, swapped letters or punctuation | `fuzzy::close_handles`, `fuzzy::osa_within`, `fuzzy::jaro_winkler` |
| Sweeping a dump, paste or page for a whole watch list in one pass | `textsearch::PatternSet`, `textsearch::Scanner` |
| Near-misses of a whole watch list in a dump, a few lookups per word | `fuzzy::NearSet`, `fuzzy::NearSearcher` |
| Closest approach between sightings | `geo::closest_pair_geo` |
| Which sightings fall inside an area of interest | `geo::Fence` |
| The area a set of sightings covers | `geo::convex_hull`, `geo::polygon_area_m2` |
| Street blocks equally close to two places | `geo::taxicab_ties` |
| Event counts and peaks over time windows | `osint::TimeBuckets` |
| Bursts and silences in a feed | `osint::burst`, `osint::silences`, `osint::longest_silence` |
| The quickest swing between a series' lowest and highest values | `arrays::shortest_min_max_window` |
| Top-k lists | `heap::top_k_largest` |
| Near-duplicate images by perceptual hash | `osint::similar_hashes` |
| Username sweeps | `osint::username_candidates` |
| Pattern of life (days × hours grid) | `matrix::largest_rectangle` |
| Infrastructure two targets share | `arrays::intersect_counted`, `arrays::intersect_sorted`, `arrays::intersect_unique` |
| Address blocks nested in larger blocks | `arrays::contained_intervals` |

## Your list, item by item

"Checked" means the function was compared with CS Academy's own statement, quoted in a public repository of
solutions (see Sources consulted). The other items follow what the name usually means. "My reading" marks the
ones where that meaning is a real guess.

| Item | Function | Note |
|---|---|---|
| BFS | `graph::bfs`, `Bfs::path_to`, `Bfs::within`, `graph::shortest_path` | |
| DFS | `graph::dfs_order` | iterative: no stack overflow on deep graphs |
| Topological sorting | `graph::topological_sort` | |
| Stack ops | `stack::balanced`, `stack::next_greater` | |
| Disjoint set | `dsu::DisjointSet` | |
| Heap operations | `heap::MinHeap` (`peek`, `push`, `pop`; `sift_up`, `sift_down` inside), `heap::top_k_largest` | |
| Fenwick trees | `fenwick::Fenwick` | why O(log N) matters: module docs and `examples/growth.rs` |
| Merge K sorted lists | `merge::merge_sorted`, `merge::merge_k_sorted_lists` | |
| Segment trees | `segtree::SegTree` with `Sum`, `Min`, `Max` | |
| Binary search | `search::partition_point`, `lower_bound`, `upper_bound`, `find`, `count_in_range` | |
| Selection, insertion, bubble sort | `sort::selection_sort`, `insertion_sort`, `bubble_sort` | |
| Merge sort | `sort::merge_sort`, `sort::merge_sort_by` | stable |
| Complexity analysis | `complexity` module, `examples/growth.rs` | |
| Anagrams | `arrays::largest_anagram_group`, `arrays::is_anagram`, `arrays::group_anagrams` | checked: the task asks for `largest_anagram_group` |
| Longest parenthesised substring | `stack::longest_balanced_run` | checked: the task asks for the run's length |
| Array intersection | `arrays::intersect_counted`, `arrays::intersect_sorted`, `arrays::intersect_unique` | checked: the task is `intersect_counted`, and sorting first then `intersect_sorted` is its low-memory version |
| Previous permutation | `combinatorics::prev_permutation` | |
| Contained intervals | `arrays::contained_intervals` | my reading |
| Odd divisors | `numtheory::count_odd_divisors`, `numtheory::sum_of_greatest_odd_divisors` | my reading: both common versions |
| Donkey Paradox | `geo::taxicab_ties` | checked |
| Consecutive max difference | `arrays::max_gap` | my reading: largest gap after sorting, in O(N) |
| Bounded offset sorting | `sort::sort_bounded_offset` | |
| Permutations with duplicates | `combinatorics::unique_permutations` | |
| Optimal digits removal | `stack::smallest_after_removing` | |
| Matrix rotations | `matrix::rotate_clockwise`, `rotate_anticlockwise`, `rotate_square_in_place` | one public solution suggests CS Academy's task overlays all four turns of a 0/1 grid |
| 3-divisible pairs | `arrays::count_pairs_divisible_by_three` | checked |
| Binary stable sort | `sort::binary_stable_sort` | |
| Odd divisor count | `numtheory::count_with_odd_divisor_count` | my reading: numbers with an odd number of divisors; two public solutions agree |
| Word permutations | `combinatorics::word_permutations` | my reading: every rearrangement of a word. Two public solutions suggest CS Academy's task sorts a list of words instead |
| Palindrome creation | `arrays::palindrome_from_letters` | my reading: rearrange the letters |
| LSB | `bits::lowest_set_bit`, `lowest_set_bit_index`, `clear_lowest_set_bit` | |
| All BST preorders | `combinatorics::bst_preorders` | |
| Largest concat number | `combinatorics::largest_concatenation` | |
| Power functions | `numtheory::pow_checked`, `numtheory::pow_mod` | |
| Look and say | `combinatorics::look_and_say`, `look_and_say_sequence`, `arrays::run_lengths` | |
| Array MEX | `arrays::mex` | |
| Find any duplicate | `arrays::find_any_duplicate`, `arrays::find_duplicate_floyd` | |
| Generate combinations | `combinatorics::combinations`, `combinations_of` | |
| Largest square | `matrix::largest_square` | |
| Bit count | `bits::bit_count` | |
| Fibonacci sum | `numtheory::fibonacci_sum`, `numtheory::fibonacci` | |
| Generate parentheses | `combinatorics::balanced_parentheses` | |
| Closest pair | `geo::closest_pair`, `geo::closest_pair_geo` | my reading: points |
| Find circular permutation | `bits::gray_code_cycle` | my reading: one-bit-step cycle |
| Largest matrix zone | `matrix::largest_zone`, `matrix::largest_rectangle` | my reading: largest connected patch |
| Min max subarray | `arrays::shortest_min_max_window` | checked. My first reading, the largest and smallest sums, was wrong; `max_subarray` and `min_subarray` stay, for bursts |

### Left out

Puzzles with no use for SkeleTrace came out on 10 October: sudoku, tic-tac-toe, banknotes, domino tilings and
the product of five. The 8 October zip still has them. Pokémon evolutions and Single Letter (CS Academy calls it
One Letter) are the same kind of puzzle, so they weren't built, although their statements were found.

Not built, because their statements couldn't be read: Four X-tremes, Flips, Formatting (on CS Academy, Justify
Formatting), Inserting Rectangles, No Gaps Arrays, Even Steps Elimination, Binary Matrix Changes, Pair Strange
Sorts (Pair Strange Sort), and Bit Removal beyond clearing the lowest set bit. Soldiers Row is a lesson in CS
Academy's stack course rather than a task, and its text couldn't be read either.

## Changes

- 0.4.0, 10 October 2026: added `fuzzy::NearSet` and `fuzzy::NearSearcher`. They check a word against many
  terms at once, each term with its own allowance of up to two typos, in a few hash lookups (symmetric
  deletion, with a prefix limit). `fuzzy::NameMatch` now also carries `NearSet` matches; its fields are
  unchanged. The tour's fuzzy section shows a near-miss set. Nothing that was already here changed behaviour.
- 0.3.0, 10 October 2026: added the `fuzzy` module (edit distances, Jaro-Winkler, handle matching) and the
  `textsearch` module (Aho-Corasick sweeps, in chunks). Added `geo::Fence`, `geo::convex_hull`,
  `geo::polygon_area_m2`, `graph::WeightedGraph`, `graph::dijkstra`, `graph::cheapest_path` and
  `osint::strongest_chain`. `GraphError` has a new variant, `BadCost`.
- 0.2.0, 10 October 2026: added `geo::taxicab_ties`, `arrays::shortest_min_max_window`,
  `arrays::intersect_counted` and `arrays::largest_anagram_group`. Removed the `dojo` feature and its module.
- 0.1.0, 8 October 2026: the first version.

## Sources consulted

- SymSpell, by Wolf Garbe (github.com/wolfgarbe/SymSpell): the idea behind `fuzzy::NearSet`, symmetric
  deletion with a prefix limit. No code was taken.
- CS Academy (csacademy.com): its lessons and problems only show with JavaScript running, so they couldn't be
  read here. Its code and problem text belong to CS Academy in any case; everything here is written fresh.
- DianaLuca/Algorithms on GitHub, folder `CSAcademy`: its header comments quote CS Academy's statements for
  Anagrams, Array Intersection, Donkey Paradox, Longest Parenthesised Substring, Min Max Subarray, One Letter,
  Pokémon Evolution and 3-Divisible Pairs. The checked items were compared with those. No code was taken.
- SH-anonta/Competitive-Programming-Solutions and smhemel/CsAcademy-Online-Judge on GitHub: solutions with no
  statements, read only to see what Word Permutation, Matrix Rotations and Odd Divisor Count compute. No code
  was taken.
- Codeforces, CS Academy's announcement of its interview archive (codeforces.com/blog/entry/47990): the source
  of the name Justify Formatting.
- Wikipedia, "Computer and information science" and "Open-source intelligence": overview articles, with no
  algorithms or code to take.
- Reuser's Repertorium (rr.reuser.biz): an OSINT link directory, copyright Arno H.P. Reuser. No code. It is
  worth reading by hand for SkeleTrace's sources registry, but as a compiled work it shouldn't be copied
  wholesale.

## Licence

Copyright (C) 2026 whispr.dev. Every source file says so, with the SPDX line `GPL-3.0-only`.

`Cargo.toml` says GPL-3.0-only, to match the Community edition. As the owner you can license it differently
for Pro builds; change the field if you do.
