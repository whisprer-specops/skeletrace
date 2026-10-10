// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! A guided tour of skeletrace-algo: every module run on one made-up case, with what each answer means.
//!
//! ```text
//! cargo run --example tour                    every section
//! cargo run --example tour -- graphs          one section
//! cargo run --example tour -- list            the section names
//! ```
//!
//! All the data is invented. Every name, address and number comes from ranges set aside for examples
//! (example.org and .example names, the 192.0.2.0/24, 198.51.100.0/24 and 203.0.113.0/24 test networks,
//! AS64496 to AS64511, and Ofcom's 07700 900xxx drama numbers), so nothing here points at a real person.

use skeletrace_algo::graph::{KeyedGraph, TopoError};
use skeletrace_algo::{
    arrays, bits, combinatorics, complexity, fenwick, fuzzy, geo, graph, heap, matrix, merge,
    numtheory, osint, search, segtree, sort, stack, textsearch,
};
use std::error::Error;
use std::fmt::Display;
use std::net::Ipv4Addr;

type Outcome = Result<(), Box<dyn Error>>;
type Section = fn() -> Outcome;

/// The case: each pair is one link found in the evidence.
const CASE_LINKS: [(&str, &str); 11] = [
    ("lantern@example.org", "@lantern_ops"),
    ("@lantern_ops", "lantern-shop.example"),
    ("lantern-shop.example", "lantern@example.org"),
    ("lantern-shop.example", "198.51.100.23"),
    ("198.51.100.23", "AS64500"),
    ("@lantern_ops", "+44 7700 900123"),
    ("+44 7700 900123", "relay@example.net"),
    ("relay@example.net", "@moth_relay"),
    ("@moth_relay", "203.0.113.9"),
    ("203.0.113.9", "relay@example.net"),
    ("@quiet_fox", "+44 7700 900456"),
];

/// Collection steps: the first of each pair has to finish before the second can start.
const PIPELINE: [(&str, &str); 9] = [
    ("seed list", "resolve domains"),
    ("seed list", "WHOIS lookups"),
    ("resolve domains", "IP to ASN"),
    ("resolve domains", "geolocate IPs"),
    ("WHOIS lookups", "build case graph"),
    ("IP to ASN", "build case graph"),
    ("geolocate IPs", "build case graph"),
    ("build case graph", "find weak points"),
    ("find weak points", "write report"),
];

/// Evidence arriving one item at a time: two identifiers, and why they belong together.
const EVIDENCE: [(&str, &str, &str); 5] = [
    (
        "lantern@example.org",
        "@lantern_ops",
        "the same profile photo",
    ),
    (
        "@moth_relay",
        "relay@example.net",
        "a password-reset page shows r****@example.net",
    ),
    (
        "@lantern_ops",
        "+44 7700 900123",
        "the number posted in a reply",
    ),
    (
        "+44 7700 900123",
        "relay@example.net",
        "both in one WHOIS record",
    ),
    ("@quiet_fox", "+44 7700 900456", "the number in the bio"),
];

const HOUR_MS: i64 = 3_600_000;
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn main() {
    let list = sections();
    let wanted: Vec<String> = std::env::args().skip(1).collect();
    if wanted
        .iter()
        .any(|w| matches!(w.as_str(), "list" | "-h" | "--help"))
    {
        print_sections(&list);
        return;
    }
    let mut ran = 0;
    for &(name, _, run) in &list {
        if !wanted.is_empty() && !wanted.iter().any(|w| w == name) {
            continue;
        }
        if let Err(e) = run() {
            eprintln!("The {name} section stopped: {e}");
            std::process::exit(1);
        }
        ran += 1;
    }
    if ran == 0 {
        eprintln!("No section called {}.", wanted.join(" or "));
        print_sections(&list);
        std::process::exit(2);
    }
    println!();
    println!("Every answer above came from the real crate. The source is examples/tour.rs.");
}

fn sections() -> Vec<(&'static str, &'static str, Section)> {
    let base: [(&'static str, &'static str, Section); 17] = [
        (
            "graphs",
            "BFS, DFS, components, weak points, run order",
            graphs,
        ),
        (
            "clusters",
            "union-find: which identifiers are one actor",
            clusters,
        ),
        ("heaps", "min-heap, top-k, merging receiver feeds", heaps),
        (
            "sorting",
            "four sorts, late messages, a stable split",
            sorting,
        ),
        ("searching", "binary search, an IP-to-ASN table", searching),
        (
            "fuzzy",
            "handles that differ by typos, swaps or punctuation",
            fuzzy_names,
        ),
        (
            "sweep",
            "one pass over a dump for a whole watch list",
            sweep,
        ),
        ("ranges", "Fenwick tree, segment tree, time buckets", ranges),
        (
            "arrays",
            "shared infrastructure, nested blocks, silences",
            arrays_section,
        ),
        (
            "stacks",
            "brackets, the next busier day, digit removal",
            stacks,
        ),
        ("grids", "pattern of life: steady blocks and places", grids),
        (
            "geo",
            "closest approach, geofences, covered area, tied blocks",
            geography,
        ),
        (
            "bits",
            "bit counts, lowest bits, one-bit-step cycles",
            bit_tricks,
        ),
        ("numbers", "fast powers, divisors, Fibonacci", numbers),
        (
            "combinatorics",
            "permutations, combinations, look-and-say",
            combos,
        ),
        (
            "osint",
            "username sweeps, near-duplicate images, bursts, evidence chains",
            osint_helpers,
        ),
        ("complexity", "why O(log N) matters", growth),
    ];
    base.to_vec()
}

fn print_sections(list: &[(&'static str, &'static str, Section)]) {
    println!("Sections. Run one with: cargo run --example tour -- NAME");
    for (name, about, _) in list {
        println!("  {name:<14} {about}");
    }
}

/// Prints a section heading.
fn heading(title: &str, about: &str) {
    let rule = "━".repeat(78usize.saturating_sub(title.chars().count()));
    println!();
    println!("━━ {title} {rule}");
    println!("{about}");
    println!();
}

/// Prints one call and what it returned.
fn line(call: &str, result: impl Display) {
    println!("  {call}");
    println!("      → {result}");
}

/// Joins anything string-like.
fn join<S: AsRef<str>>(items: impl IntoIterator<Item = S>, sep: &str) -> String {
    let parts: Vec<String> = items.into_iter().map(|s| s.as_ref().to_owned()).collect();
    parts.join(sep)
}

fn hhmm(minutes: i64) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn clock(ms: i64) -> String {
    hhmm(ms / 60_000)
}

fn case_graph() -> KeyedGraph<&'static str> {
    let mut g = KeyedGraph::undirected();
    for (a, b) in CASE_LINKS {
        g.link(&a, &b);
    }
    g
}

fn id(g: &KeyedGraph<&'static str>, key: &'static str) -> Result<usize, Box<dyn Error>> {
    g.index_of(&key)
        .ok_or_else(|| format!("{key} is not in the case").into())
}

fn label(g: &KeyedGraph<&'static str>, i: usize) -> &'static str {
    g.key(i).copied().unwrap_or("?")
}

fn labels(g: &KeyedGraph<&'static str>, nodes: &[usize]) -> String {
    join(nodes.iter().map(|&i| label(g, i)), ", ")
}

/// The case's groups after taking one identifier out, leaving out the identifier itself.
fn components_without(
    case: &KeyedGraph<&'static str>,
    removed: usize,
) -> Result<Vec<Vec<usize>>, Box<dyn Error>> {
    let mut rest = graph::Graph::undirected(case.graph().node_count());
    for (a, b) in CASE_LINKS {
        let (x, y) = (id(case, a)?, id(case, b)?);
        if x != removed && y != removed {
            rest.add_edge(x, y)?;
        }
    }
    Ok(graph::components(&rest)
        .into_iter()
        .filter(|group| group != &[removed])
        .collect())
}

fn graphs() -> Outcome {
    heading(
        "Graphs",
        "Who links to whom. The case: eleven links between eleven identifiers.",
    );
    let case = case_graph();
    let g = case.graph();
    for (a, b) in CASE_LINKS {
        println!("    {a:<22} ── {b}");
    }
    println!();

    let start = id(&case, "lantern@example.org")?;
    let path = graph::shortest_path(g, start, id(&case, "@moth_relay")?)?.ok_or("no path")?;
    line(
        "graph::shortest_path(g, lantern@example.org, @moth_relay)   breadth-first search",
        format!(
            "{}   ({} hops)",
            join(path.iter().map(|&i| label(&case, i)), " → "),
            path.len() - 1
        ),
    );
    let from_start = graph::bfs(g, start)?;
    for key in ["203.0.113.9", "@quiet_fox"] {
        let hops = from_start.distance(id(&case, key)?);
        line(
            &format!("graph::bfs(g, lantern@example.org)?.distance({key})"),
            hops.map_or("None: not connected at all".to_string(), |d| {
                format!("{d} hops")
            }),
        );
    }
    let phone = id(&case, "+44 7700 900123")?;
    let near = graph::bfs(g, phone)?.within(1);
    line(
        "graph::bfs(g, +44 7700 900123)?.within(1)   one hop out",
        labels(&case, &near[1..]),
    );
    let order = graph::dfs_order(g, start)?;
    line(
        "graph::dfs_order(g, lantern@example.org)   depth-first: as deep as it can, then back up",
        labels(&case, &order),
    );

    let groups = graph::components(g);
    println!("  graph::components(g)");
    println!("      → {} separate groups", groups.len());
    for group in &groups {
        println!("        {}", labels(&case, group));
    }

    let weak = graph::weak_points(g)?;
    println!("  graph::weak_points(g)   what a single loss would cut");
    println!("      → cut nodes: {}", labels(&case, &weak.cut_nodes));
    let bridges: Vec<String> = weak
        .bridges
        .iter()
        .map(|&(a, b)| format!("{} ── {}", label(&case, a), label(&case, b)))
        .collect();
    println!("      → bridges:   {}", bridges.join(" | "));
    let original = groups
        .iter()
        .find(|group| group.contains(&phone))
        .ok_or("the phone has no group")?;
    println!("  Take +44 7700 900123 away and its group falls apart:");
    for group in components_without(&case, phone)?
        .iter()
        .filter(|group| original.contains(&group[0]))
    {
        println!("        {}", labels(&case, group));
    }
    println!("  That number is the only thread between the Lantern side and the Moth side: check it first.");
    println!();

    let mut plan = KeyedGraph::directed();
    for (before, after) in PIPELINE {
        plan.link(&before, &after);
    }
    let order = graph::topological_sort(plan.graph())?;
    line(
        "graph::topological_sort(plan)   collection steps, each after the steps it needs",
        join(order.iter().map(|&i| label(&plan, i)), " → "),
    );
    plan.link(&"write report", &"seed list");
    match graph::topological_sort(plan.graph()) {
        Err(TopoError::Cycle(cycle)) => {
            let first = cycle.first().map_or("?", |&i| label(&plan, i));
            line(
                "...then add write report → seed list",
                format!(
                    "refused, it goes round in a circle: {} → back to {first}",
                    join(cycle.iter().map(|&i| label(&plan, i)), " → ")
                ),
            );
        }
        other => return Err(format!("expected a cycle, got {other:?}").into()),
    }
    Ok(())
}

fn clusters() -> Outcome {
    heading(
        "Clusters",
        "Union-find: each piece of evidence ties two identifiers together, and every chain of ties is one actor.",
    );
    let mut actors = osint::EntityClusters::new();
    for (a, b, why) in EVIDENCE {
        let joined = actors.link(&a, &b);
        line(
            &format!("actors.link({a}, {b})   evidence: {why}"),
            format!(
                "{}, {} actor(s) so far",
                if joined {
                    "joined"
                } else {
                    "already one actor"
                },
                actors.cluster_count()
            ),
        );
    }
    println!("  actors.clusters()");
    for (n, group) in actors.clusters().iter().enumerate() {
        println!("      → actor {}: {}", n + 1, group.join(", "));
    }
    for (a, b) in [
        ("lantern@example.org", "@moth_relay"),
        ("@lantern_ops", "@quiet_fox"),
    ] {
        line(
            &format!("actors.same_actor({a}, {b})"),
            actors.same_actor(&a, &b),
        );
    }
    println!("  Underneath is dsu::DisjointSet: each link costs about the same however long the chain gets.");
    Ok(())
}

fn heaps() -> Outcome {
    heading(
        "Heaps and merging",
        "A min-heap always knows its smallest item: O(log N) to add or remove one, O(1) to look.",
    );
    let mut h = heap::MinHeap::new();
    for x in [42, 7, 19, 3, 25, 11] {
        h.push(x);
        println!(
            "  h.push({x:>2})   → h.peek() is now {}",
            h.peek().copied().unwrap_or_default()
        );
    }
    let mut popped = Vec::new();
    while let Some(x) = h.pop() {
        popped.push(x.to_string());
    }
    line("h.pop() until empty", popped.join(", "));

    let case = case_graph();
    let g = case.graph();
    let mut links = Vec::new();
    for i in 0..g.node_count() {
        links.push((g.degree(i)?, label(&case, i)));
    }
    let busiest = heap::top_k_largest(links, 3);
    line(
        "heap::top_k_largest(links per identifier, 3)",
        join(
            busiest.iter().map(|(n, key)| format!("{key} ({n} links)")),
            ", ",
        ),
    );

    let receivers = [
        vec![
            (3, 'A', "LANTERN1"),
            (17, 'A', "MOTH2"),
            (42, 'A', "LANTERN1"),
        ],
        vec![(5, 'B', "FOX3"), (17, 'B', "MOTH2"), (60, 'B', "FOX3")],
        vec![(1, 'C', "LANTERN1"), (30, 'C', "MOTH2")],
    ];
    println!("  merge::merge_sorted(three receivers, each already in time order)");
    for (secs, receiver, callsign) in merge::merge_sorted(receivers) {
        println!(
            "      → 12:{:02}:{:02}  receiver {receiver}  {callsign}",
            secs / 60,
            secs % 60
        );
    }
    Ok(())
}

fn sorting() -> Outcome {
    heading(
        "Sorting",
        "Four ways to sort one list. They agree on the answer and differ in the work it takes.",
    );
    let data = [29, 3, 17, 8, 23, 1, 12, 5, 30, 14, 9, 21];
    println!("  input {data:?}");
    type Sorter = fn(&mut [i32]);
    let sorters: [(&str, Sorter); 4] = [
        ("sort::selection_sort", sort::selection_sort),
        ("sort::insertion_sort", sort::insertion_sort),
        ("sort::bubble_sort", sort::bubble_sort),
        ("sort::merge_sort", sort::merge_sort),
    ];
    for (name, sorter) in sorters {
        let mut v = data;
        sorter(&mut v);
        line(name, format!("{v:?}"));
    }
    let late = vec![1, 3, 2, 4, 7, 5, 6, 8, 11, 9, 10, 12];
    line(
        "sort::sort_bounded_offset(AIS messages, none more than 2 places late, 2)   O(N log k)",
        format!("{:?}", sort::sort_bounded_offset(late, 2)),
    );
    let messages = vec![
        ("m1", false),
        ("m2", true),
        ("m3", false),
        ("m4", true),
        ("m5", false),
    ];
    let ordered = sort::binary_stable_sort(messages, |m| m.1);
    line(
        "sort::binary_stable_sort(messages, needs translation?)   order kept within each group",
        join(ordered.iter().map(|m| m.0), ", "),
    );
    Ok(())
}

fn searching() -> Outcome {
    heading("Searching", "Binary search halves what is left at every look: about 30 looks for a billion sorted items.");
    let times: [i64; 10] = [425, 520, 542, 555, 598, 630, 825, 840, 980, 1330];
    println!(
        "  event times {}",
        join(times.iter().map(|&m| hhmm(m)), " ")
    );
    line(
        "search::count_in_range(times, 09:00, 10:00)",
        search::count_in_range(&times, &540, &600),
    );
    let i = search::lower_bound(&times, &780);
    line(
        "search::lower_bound(times, 13:00)",
        format!(
            "index {i}: the first event at or after 13:00 is at {}",
            times.get(i).map_or("none".to_string(), |&m| hhmm(m))
        ),
    );
    line(
        "search::find(times, 14:00)",
        format!("{:?}", search::find(&times, &840)),
    );

    let blocks = [
        (Ipv4Addr::new(198, 51, 100, 0), 24, "AS64500"),
        (Ipv4Addr::new(203, 0, 113, 0), 24, "AS64501"),
        (Ipv4Addr::new(192, 0, 2, 0), 25, "AS64502"),
    ];
    let mut ranges = Vec::new();
    for (network, prefix, asn) in blocks {
        let (first, last) = search::cidr_bounds(network, prefix)?;
        ranges.push((first, last, asn));
    }
    let table = search::Ipv4RangeTable::new(ranges)?;
    println!(
        "  table = search::Ipv4RangeTable::new(198.51.100.0/24, 203.0.113.0/24, 192.0.2.0/25)"
    );
    for ip in [
        Ipv4Addr::new(198, 51, 100, 23),
        Ipv4Addr::new(203, 0, 113, 9),
        Ipv4Addr::new(192, 0, 2, 7),
        Ipv4Addr::new(192, 0, 2, 200),
    ] {
        line(
            &format!("table.lookup({ip})"),
            table
                .lookup(ip)
                .copied()
                .unwrap_or("no match: outside every block"),
        );
    }
    for n in [10, 1_000_000_000] {
        line(
            &format!("complexity::binary_search_steps({n})"),
            complexity::binary_search_steps(n),
        );
    }
    Ok(())
}

fn fuzzy_names() -> Outcome {
    heading(
        "Fuzzy matching",
        "Handles and names that differ by typos, swapped letters or punctuation.",
    );
    line(
        "fuzzy::levenshtein(\"jdoe88\", \"jdeo88\")   inserts, deletes, substitutions",
        fuzzy::levenshtein("jdoe88", "jdeo88"),
    );
    line(
        "fuzzy::osa_distance(\"jdoe88\", \"jdeo88\")   a swapped pair is one typo",
        fuzzy::osa_distance("jdoe88", "jdeo88"),
    );
    line(
        "fuzzy::levenshtein_within(\"lantern_ops\", \"moth_relay\", 2)   gives up early",
        format!(
            "{:?}",
            fuzzy::levenshtein_within("lantern_ops", "moth_relay", 2)
        ),
    );
    line(
        "fuzzy::jaro_winkler(\"MARTHA\", \"MARHTA\")   a shared start counts for more",
        format!("{:.3}", fuzzy::jaro_winkler("MARTHA", "MARHTA")),
    );
    line(
        "fuzzy::handle_key(\"@Lantern_Ops\")",
        fuzzy::handle_key("@Lantern_Ops"),
    );
    let seen = [
        "lantern.ops",
        "lanternops1",
        "latnern_ops",
        "lantern_shop",
        "moth_relay",
        "LanternOps",
        "l4ntern_ops",
    ];
    println!("  handles found on another platform: {}", seen.join(", "));
    let close = fuzzy::close_handles("@Lantern_Ops", &seen, 1);
    line(
        "fuzzy::close_handles(\"@Lantern_Ops\", those, 1)   closest first",
        join(
            close.iter().map(|m| {
                format!(
                    "{} ({} edit{})",
                    seen[m.index],
                    m.edits,
                    if m.edits == 1 { "" } else { "s" }
                )
            }),
            ", ",
        ),
    );
    // A whole watch list at once: each word costs a few lookups, however long the list.
    let watch = [
        "lantern@example.org",
        "@lantern_ops",
        "relay@example.net",
        "@moth_relay",
    ];
    let near = fuzzy::NearSet::new(watch.iter().map(|&term| (term, 2)))?;
    println!(
        "  a near-miss set of {} terms, up to 2 typos each: {}",
        near.len(),
        watch.join(", ")
    );
    let mut searcher = near.searcher();
    let mut found = Vec::new();
    for word in [
        "lantern@exmaple.org",
        "@latnern_ops",
        "relay@example.net",
        "@moth_rely",
        "@quiet_fox",
    ] {
        found.clear();
        searcher.find(word, &mut found);
        let verdict = if found.is_empty() {
            "near nothing".to_string()
        } else {
            join(
                found.iter().map(|m| {
                    format!(
                        "{} ({} edit{})",
                        watch[m.index],
                        m.edits,
                        if m.edits == 1 { "" } else { "s" }
                    )
                }),
                ", ",
            )
        };
        line(&format!("NearSet searcher.find(\"{word}\")"), verdict);
    }
    Ok(())
}

fn sweep() -> Outcome {
    heading(
        "Text sweeps",
        "One pass over a dump finds every identifier on a watch list, however long the list.",
    );
    let watch = [
        "lantern@example.org",
        "relay@example.net",
        "@moth_relay",
        "+44 7700 900123",
        "lantern-shop.example",
        "@quiet_fox",
    ];
    let dump = "2026-09-30 user=LANTERN@EXAMPLE.ORG ip=198.51.100.23 phone=+44 7700 900123\n\
                2026-09-30 user=someone@example.com ip=203.0.113.50\n\
                2026-10-01 reply from @Moth_Relay: try relay@example.net, or lantern-shop.example\n";
    println!("  watch list: {}", watch.join(", "));
    println!("  the dump:");
    for row in dump.lines() {
        println!("    {row}");
    }
    let set = textsearch::PatternSet::new_ascii_case_insensitive(watch)?;
    println!("  textsearch::PatternSet::new_ascii_case_insensitive(watch).find_all(dump)");
    for hit in set.find_all(dump) {
        println!(
            "      → byte {:>3}: {:<22} written as {}",
            hit.start,
            watch[hit.pattern],
            &dump[hit.start..hit.end]
        );
    }
    let missing: Vec<&str> = {
        let found = set.present(dump);
        (0..watch.len())
            .filter(|i| !found.contains(i))
            .map(|i| watch[i])
            .collect()
    };
    line("set.present(dump), and what's missing", missing.join(", "));
    let mut scanner = set.scanner();
    let mut hits = Vec::new();
    for chunk in dump.as_bytes().chunks(16) {
        scanner.feed(chunk, &mut hits);
    }
    if hits != set.find_all(dump) {
        return Err("reading in chunks gave different hits".into());
    }
    let split = hits
        .iter()
        .filter(|h| h.start / 16 != (h.end - 1) / 16)
        .count();
    line(
        "set.scanner(), fed 16 bytes at a time   how a dump too big for memory is read",
        format!(
            "the same {} hits, {split} of them split across two chunks",
            hits.len()
        ),
    );
    Ok(())
}

fn ranges() -> Outcome {
    heading(
        "Range queries",
        "Totals and peaks over time windows while the counts keep changing: O(log N) per update and per question.",
    );
    let mut day = osint::TimeBuckets::new(0, HOUR_MS, 24).ok_or("bad bucket settings")?;
    let events = [
        (0, 12),
        (2, 1),
        (8, 30),
        (9, 5),
        (9, 40),
        (10, 15),
        (14, 2),
        (14, 20),
        (14, 25),
        (14, 50),
        (15, 5),
        (15, 30),
        (16, 45),
        (23, 59),
    ];
    for (hour, minute) in events {
        day.record(hour * HOUR_MS + minute * 60_000, 1);
    }
    let counts: Vec<i64> = (0..day.len()).map(|b| day.count(b).unwrap_or(0)).collect();
    println!(
        "  events per hour, 00 to 23: {}",
        join(counts.iter().map(|c| c.to_string()), " ")
    );
    line(
        "day.count_between(09:00, 17:00)",
        day.count_between(9 * HOUR_MS, 17 * HOUR_MS),
    );
    line(
        "day.busiest_between(00:00, 24:00)",
        day.busiest_between(0, 24 * HOUR_MS)
            .map_or("no data".to_string(), |n| format!("{n} events in one hour")),
    );

    let mut totals = fenwick::Fenwick::from_slice(&counts);
    let all = totals.prefix_sum(counts.len());
    let hour = totals.lower_bound((all + 1) / 2);
    line(
        "fenwick::Fenwick::from_slice(counts).lower_bound(half the events)",
        format!("half of the day's {all} events had happened by {hour:02}:59"),
    );
    totals.add(9, 6);
    line(
        "totals.add(09:00 bucket, 6), then totals.range_sum(08:00, 12:00)",
        totals.range_sum(8, 12),
    );

    let mut peaks = segtree::SegTree::<segtree::Max>::from_slice(&counts);
    line(
        "segtree::SegTree::<Max>::from_slice(counts).query(08:00, 12:00)",
        peaks.query(8, 12),
    );
    peaks.update(10, 9);
    line(
        "peaks.update(10:00 bucket, 9), then peaks.query(08:00, 12:00)",
        peaks.query(8, 12),
    );
    Ok(())
}

fn arrays_section() -> Outcome {
    heading("Arrays", "Shared infrastructure, nested address blocks, silences, standout runs, and some quick ones.");
    let shop = [
        "ns2.hosting.example",
        "203.0.113.9",
        "198.51.100.23",
        "ns1.hosting.example",
    ];
    let relay = [
        "ns1.hosting.example",
        "198.51.100.77",
        "ns3.other.example",
        "203.0.113.9",
    ];
    line(
        "arrays::intersect_counted(the shop's infrastructure, the relay's)   in lookup order",
        join(arrays::intersect_counted(&shop, &relay), ", "),
    );
    let (mut shop_sorted, mut relay_sorted) = (shop, relay);
    shop_sorted.sort_unstable();
    relay_sorted.sort_unstable();
    line(
        "arrays::intersect_sorted(both lists sorted first)   no extra memory needed",
        join(arrays::intersect_sorted(&shop_sorted, &relay_sorted), ", "),
    );

    let netblocks = [
        ("198.51.100.0", 24),
        ("198.51.100.0", 26),
        ("198.51.100.64", 26),
        ("203.0.113.0", 24),
        ("203.0.113.128", 25),
    ];
    let mut spans = Vec::new();
    for (network, prefix) in netblocks {
        let (first, last) = search::cidr_bounds(network.parse()?, prefix)?;
        spans.push((i64::from(u32::from(first)), i64::from(u32::from(last))));
    }
    let inside = arrays::contained_intervals(&spans);
    line(
        "arrays::contained_intervals(netblocks)   blocks that sit inside a bigger one",
        join(
            inside
                .iter()
                .map(|&i| format!("{}/{}", netblocks[i].0, netblocks[i].1)),
            ", ",
        ),
    );

    let seen: [i64; 7] = [605, 610, 640, 641, 900, 905, 1300];
    let longest = arrays::max_gap(&seen).unwrap_or(0);
    line(
        "arrays::max_gap(event minutes)   the longest silence, found without sorting",
        format!("{} h {} min", longest / 60, longest % 60),
    );
    let change = [3, -2, 5, 1, -7, -3, 2, 8, -1];
    if let (Some(best), Some(worst)) =
        (arrays::max_subarray(&change), arrays::min_subarray(&change))
    {
        line(
            "arrays::max_subarray(daily follower change)",
            format!("days {}..{}, {:+}", best.start, best.end, best.sum),
        );
        line(
            "arrays::min_subarray(daily follower change)",
            format!("days {}..{}, {:+}", worst.start, worst.end, worst.sum),
        );
    }
    let followers = [1210, 1180, 1530, 1095, 1240, 1720, 1600, 1095, 1350];
    let swing = arrays::shortest_min_max_window(&followers).ok_or("no counts")?;
    line(
        "arrays::shortest_min_max_window(daily follower counts)   the quickest full swing",
        format!(
            "days {}..{}: {} to {} within {} days",
            swing.start,
            swing.end,
            followers[swing.start],
            followers[swing.end - 1],
            swing.len()
        ),
    );

    line(
        "arrays::mex([0, 1, 2, 4, 5])   the lowest unused case number",
        arrays::mex(&[0, 1, 2, 4, 5]),
    );
    let handles = ["@lantern_ops", "@moth_relay", "@quiet_fox", "@moth_relay"];
    line(
        "arrays::find_any_duplicate(handles)",
        arrays::find_any_duplicate(&handles)
            .copied()
            .unwrap_or("none"),
    );
    line(
        "arrays::find_duplicate_floyd([1, 3, 4, 2, 2])   with O(1) extra memory",
        format!("{:?}", arrays::find_duplicate_floyd(&[1, 3, 4, 2, 2])),
    );
    line(
        "arrays::count_pairs_divisible_by_three([1, 2, 3, 4, 5, 6])",
        arrays::count_pairs_divisible_by_three(&[1, 2, 3, 4, 5, 6]),
    );
    let words = [
        "tinsel", "listen", "silent", "moth", "thom", "enlist", "harbor",
    ];
    let groups: Vec<String> = arrays::group_anagrams(&words)
        .iter()
        .map(|group| format!("[{}]", group.join(" ")))
        .collect();
    line("arrays::group_anagrams(handles)", groups.join(" "));
    line(
        "arrays::largest_anagram_group(handles)",
        arrays::largest_anagram_group(&words),
    );
    line(
        "arrays::palindrome_from_letters(\"racecar\")",
        format!("{:?}", arrays::palindrome_from_letters("racecar")),
    );
    line(
        "arrays::run_lengths(on/off activity)",
        format!("{:?}", arrays::run_lengths(&[0, 0, 1, 1, 1, 0, 1, 1])),
    );
    Ok(())
}

fn stacks() -> Outcome {
    heading(
        "Stacks",
        "Last in, first out: a stack remembers what is still open, or still waiting for an answer.",
    );
    for text in ["{\"ids\": [1, (2)]}", "([)]"] {
        line(&format!("stack::balanced({text:?})"), stack::balanced(text));
    }
    let log = ")(()())x((";
    let run = stack::longest_balanced_run(log);
    let found = &log[run.start..run.end];
    line(
        &format!("stack::longest_balanced_run({log:?})"),
        format!("bytes {}..{}: {found:?}", run.start, run.end),
    );
    let daily = [3, 1, 4, 1, 5, 9, 2, 6];
    let waits: Vec<String> = stack::next_greater(&daily)
        .iter()
        .enumerate()
        .map(|(day, next)| next.map_or("-".to_string(), |busier| (busier - day).to_string()))
        .collect();
    line(
        &format!("stack::next_greater({daily:?})   days until a busier day"),
        waits.join(" "),
    );
    line(
        "stack::smallest_after_removing(\"4205123\", 3)",
        stack::smallest_after_removing("4205123", 3).unwrap_or_default(),
    );
    Ok(())
}

fn grids() -> Outcome {
    heading(
        "Grids",
        "Pattern of life: days down the side, hours across, █ where something happened.",
    );
    let mut week: Vec<Vec<bool>> = (0..7).map(|_| vec![false; 24]).collect();
    for day in week.iter_mut().take(5) {
        for (hour, cell) in day.iter_mut().enumerate() {
            *cell = (9..18).contains(&hour) && hour != 12;
        }
    }
    for cell in &mut week[5][20..24] {
        *cell = true;
    }
    week[1][2] = true;
    week[3][22] = true;
    println!("        0     6     12    18  23");
    for (day, row) in DAYS.iter().zip(&week) {
        let cells: String = row.iter().map(|&on| if on { '█' } else { '·' }).collect();
        println!("    {day} {cells}");
    }
    println!();
    let block = matrix::largest_rectangle(&week)?.ok_or("no active cells")?;
    line(
        "matrix::largest_rectangle(week)   the biggest steady block",
        format!(
            "{} to {}, {:02}:00 to {:02}:00, {} cells",
            DAYS[block.row],
            DAYS[block.row + block.height - 1],
            block.col,
            block.col + block.width,
            block.area()
        ),
    );
    let square = matrix::largest_square(&week)?.ok_or("no active cells")?;
    line(
        "matrix::largest_square(week)",
        format!(
            "{size} × {size}, from {} at {:02}:00",
            DAYS[square.row],
            square.col,
            size = square.size
        ),
    );

    let places: Vec<Vec<char>> = [
        "HHTWWTHH", "HHTWWTHH", "HHTWWTHH", "HHTWWTHH", "HHTWWTHH", "HHHHHHHH", "HHHHHHHT",
    ]
    .iter()
    .map(|row| row.chars().collect())
    .collect();
    println!("  where the phone was, in three-hour blocks (H home, W work, T travelling):");
    for (day, row) in DAYS.iter().zip(&places) {
        println!("    {day} {}", row.iter().collect::<String>());
    }
    let zone = matrix::largest_zone(&places)?.ok_or("empty grid")?;
    let place = match places[zone.row][zone.col] {
        'H' => "home",
        'W' => "work",
        _ => "travelling",
    };
    line(
        "matrix::largest_zone(places)   the biggest connected stretch of one place",
        format!(
            "{place}, {} blocks joined up, starting {} {:02}:00",
            zone.cells,
            DAYS[zone.row],
            zone.col * 3
        ),
    );
    let small = [vec![1, 2, 3], vec![4, 5, 6]];
    line(
        "matrix::rotate_clockwise([[1, 2, 3], [4, 5, 6]])",
        format!("{:?}", matrix::rotate_clockwise(&small)?),
    );
    Ok(())
}

fn geography() -> Outcome {
    heading(
        "Geography",
        "Closest approach, geofences, the area sightings cover, and street blocks where two places tie.",
    );
    let sightings = [
        ("MV Lantern", (50.7800, -1.1000)),
        ("MV Moth", (50.9500, 1.3000)),
        ("Dredger 7", (50.7815, -1.0950)),
        ("Pilot boat", (51.1000, 1.4000)),
        ("Fishing 22", (50.6000, -1.5000)),
    ];
    for (name, (lat, lon)) in sightings {
        println!("    {name:<11} {lat:.4}, {lon:.4}");
    }
    let points: Vec<(f64, f64)> = sightings.iter().map(|s| s.1).collect();
    let pair = geo::closest_pair_geo(&points).ok_or("need two sightings")?;
    line(
        "geo::closest_pair_geo(sightings)",
        format!(
            "{} and {}, {:.0} m apart",
            sightings[pair.first].0, sightings[pair.second].0, pair.distance
        ),
    );
    let london = (51.5074, -0.1278);
    let paris = (48.8566, 2.3522);
    line(
        "geo::haversine_m(London, Paris)",
        format!("{:.0} km", geo::haversine_m(london, paris) / 1000.0),
    );

    // An area of interest round the Solent approaches, as (latitude, longitude) corners.
    let solent = [
        (50.70, -1.35),
        (50.85, -1.35),
        (50.85, -0.95),
        (50.70, -0.95),
    ];
    let fence = geo::Fence::new(&solent).ok_or("a fence needs three corners")?;
    println!("  fence = geo::Fence::new(the Solent approaches: 50.70 to 50.85 N, 1.35 to 0.95 W)");
    for (name, at) in sightings {
        line(
            &format!("fence.locate({name})"),
            format!("{:?}", fence.locate(at)),
        );
    }
    let hull = geo::convex_hull(&points).ok_or("no sightings")?;
    let outline: Vec<(f64, f64)> = hull.iter().map(|&i| points[i]).collect();
    line(
        "geo::convex_hull(sightings)   the outline round them all",
        join(hull.iter().map(|&i| sightings[i].0), " → "),
    );
    let area = geo::polygon_area_m2(&outline).ok_or("the outline needs three corners")?;
    line(
        "geo::polygon_area_m2(that outline)",
        format!("{:.0} km² inside it", area / 1e6),
    );
    println!();

    let (rows, cols) = (7u64, 13u64);
    let (a, b) = ((1u64, 2u64), (5u64, 10u64));
    println!(
        "  A street grid, {rows} × {cols} blocks. = marks the blocks as near to A as to B on foot:"
    );
    let mut marked: u128 = 0;
    for r in 0..rows {
        let mut row = String::new();
        for c in 0..cols {
            let to_a = r.abs_diff(a.0) + c.abs_diff(a.1);
            let to_b = r.abs_diff(b.0) + c.abs_diff(b.1);
            row.push(if (r, c) == a {
                'A'
            } else if (r, c) == b {
                'B'
            } else if to_a == to_b {
                marked += 1;
                '='
            } else {
                '·'
            });
        }
        println!("    {row}");
    }
    let ties = geo::taxicab_ties(rows, cols, a, b).ok_or("A or B is off the grid")?;
    if ties != marked {
        return Err(format!("taxicab_ties said {ties}, but the drawing has {marked}").into());
    }
    line(
        &format!("geo::taxicab_ties({rows}, {cols}, A, B)   counted without visiting a block"),
        format!("{ties} blocks, the same as the = marks"),
    );
    let side = 1_000_000_000;
    let corners = geo::taxicab_ties(side, side, (0, 0), (side - 1, side - 1))
        .ok_or("a corner is off the grid")?;
    line(
        "geo::taxicab_ties(a billion × a billion, opposite corners)",
        format!("{corners}: the diagonal between the other two corners, instantly"),
    );
    Ok(())
}

fn bit_tricks() -> Outcome {
    heading(
        "Bits",
        "Counting and isolating bits, and walking every value one bit-flip at a time.",
    );
    let x: u64 = 0b1011_0100;
    line("bits::bit_count(0b1011_0100)", bits::bit_count(x));
    line(
        "bits::lowest_set_bit(0b1011_0100)",
        format!("{:#b}", bits::lowest_set_bit(x)),
    );
    line(
        "bits::lowest_set_bit_index(0b1011_0100)",
        format!("{:?}", bits::lowest_set_bit_index(x)),
    );
    line(
        "bits::clear_lowest_set_bit(0b1011_0100)",
        format!("{:#b}", bits::clear_lowest_set_bit(x)),
    );
    line(
        "bits::set_bit_positions(0b1011_0100)",
        format!("{:?}", bits::set_bit_positions(x).collect::<Vec<_>>()),
    );
    let photo: u64 = 0xF0F0_F0F0_0F0F_0F0F;
    line(
        "bits::hamming_distance(a photo's hash, the same photo resized)",
        format!(
            "{} bits differ out of 64",
            bits::hamming_distance(photo, photo ^ 0b101)
        ),
    );
    let cycle = bits::gray_code_cycle(3, 0).ok_or("bad settings")?;
    line(
        "bits::gray_code_cycle(3, 0)   all 8 values, one bit changing per step, then back to the start",
        join(cycle.iter().map(|v| format!("{v:03b}")), " → "),
    );
    Ok(())
}

fn numbers() -> Outcome {
    heading(
        "Numbers",
        "Fast powers, divisors and Fibonacci, with overflow caught instead of wrapped round.",
    );
    let small = |v: Option<u64>| v.map_or("None".to_string(), |n| n.to_string());
    let big = |v: Option<u128>| v.map_or("None".to_string(), |n| n.to_string());
    line(
        "numtheory::pow_mod(7, 128, 13)   7 squarings instead of 128 multiplications",
        small(numtheory::pow_mod(7, 128, 13)),
    );
    line(
        "numtheory::pow_checked(2, 64)   too big for 64 bits, so None rather than a wrong answer",
        small(numtheory::pow_checked(2, 64)),
    );
    line(
        "numtheory::isqrt(1_000_000_000_000)",
        numtheory::isqrt(1_000_000_000_000),
    );
    line("numtheory::gcd(84, 36)", numtheory::gcd(84, 36));
    line(
        "numtheory::count_odd_divisors(45)   1, 3, 5, 9, 15 and 45",
        small(numtheory::count_odd_divisors(45)),
    );
    line(
        "numtheory::count_with_odd_divisor_count(1, 100)   exactly the perfect squares",
        numtheory::count_with_odd_divisor_count(1, 100),
    );
    line(
        "numtheory::sum_of_greatest_odd_divisors(10)",
        numtheory::sum_of_greatest_odd_divisors(10),
    );
    line("numtheory::fibonacci(90)", big(numtheory::fibonacci(90)));
    line(
        "numtheory::fibonacci_sum(10)   0 + 1 + 1 + 2 + … + 55",
        big(numtheory::fibonacci_sum(10)),
    );
    Ok(())
}

fn combos() -> Outcome {
    heading(
        "Combinatorics",
        "Walking through arrangements and choices in a fixed order.",
    );
    let mut p = [1, 2, 3];
    let mut seen = vec![format!("{p:?}")];
    while combinatorics::next_permutation(&mut p) {
        seen.push(format!("{p:?}"));
    }
    line(
        "combinatorics::next_permutation, from [1, 2, 3] until it wraps round",
        seen.join(" "),
    );
    let mut q = [3, 1, 2];
    combinatorics::prev_permutation(&mut q);
    line(
        "combinatorics::prev_permutation([3, 1, 2])",
        format!("{q:?}"),
    );
    line(
        "combinatorics::unique_permutations([1, 1, 2])",
        format!("{:?}", combinatorics::unique_permutations(&[1, 1, 2])),
    );
    let sources = ["WHOIS", "DNS", "certificates", "socials"];
    let pairs = combinatorics::combinations_of(&sources, 2);
    line(
        "combinatorics::combinations_of(4 sources, 2)   every pair to cross-check",
        join(pairs.iter().map(|pair| pair.join("+")), ", "),
    );
    line(
        "combinatorics::balanced_parentheses(3)",
        combinatorics::balanced_parentheses(3).join(" "),
    );
    line(
        "combinatorics::bst_preorders(3)",
        format!("{:?}", combinatorics::bst_preorders(3)),
    );
    line(
        "combinatorics::bst_preorders(7).len()",
        combinatorics::bst_preorders(7).len(),
    );
    line(
        "combinatorics::look_and_say_sequence(\"1\", 7)",
        combinatorics::look_and_say_sequence("1", 7).join(", "),
    );
    line(
        "combinatorics::largest_concatenation([3, 30, 34, 5, 9])",
        combinatorics::largest_concatenation(&[3, 30, 34, 5, 9]),
    );
    let words = combinatorics::word_permutations("moth");
    line(
        "combinatorics::word_permutations(\"moth\")",
        format!("{} of them, starting {}", words.len(), words[..5].join(" ")),
    );
    Ok(())
}

fn osint_helpers() -> Outcome {
    heading(
        "OSINT helpers",
        "Built from the pieces above, for jobs SkeleTrace does every day.",
    );
    let names = osint::username_candidates(&["Jane", "Doe"]);
    println!("  osint::username_candidates([\"Jane\", \"Doe\"])");
    println!("      → {} candidates to try:", names.len());
    for row in names.chunks(6) {
        println!(
            "        {}",
            join(row.iter().map(|n| format!("{n:<11}")), "")
        );
    }
    let hashes: [u64; 4] = [
        0xF0F0_F0F0_0F0F_0F0F,
        0xF0F0_F0F0_0F0F_0F0A,
        0x0F0F_0F0F_F0F0_F0F0,
        0xF0F0_F0F0_0F0F_0E0F,
    ];
    println!("  osint::similar_hashes(four image hashes, at most 4 bits apart)");
    for (a, b, apart) in osint::similar_hashes(&hashes, 4) {
        println!(
            "      → image {a} and image {b}: {apart} bit{} apart",
            if apart == 1 { "" } else { "s" }
        );
    }
    let feed_ms: [i64; 6] = [0, 600_000, 900_000, 9_000_000, 9_300_000, 30_000_000];
    let quiet: Vec<String> = osint::silences(&feed_ms, 7_200_000)
        .iter()
        .map(|&(from, to)| format!("{} to {}", clock(from), clock(to)))
        .collect();
    line("osint::silences(feed, two hours or more)", quiet.join(", "));
    let hourly = [
        2, 1, 0, 3, 2, 1, 2, 9, 14, 11, 3, 2, 1, 2, 2, 0, 1, 3, 2, 1, 1, 0, 2, 1,
    ];
    let burst = osint::burst(&hourly, 3).ok_or("no counts")?;
    line(
        "osint::burst(posts per hour, normally 3)",
        format!(
            "{:02}:00 to {:02}:00, {} posts above normal",
            burst.start, burst.end, burst.sum
        ),
    );
    println!();

    // Evidence with a confidence each: how sure the analyst is that the link is real.
    let evidence = [
        (
            "lantern@example.org",
            "@lantern_ops",
            0.95,
            "the same profile photo",
        ),
        (
            "@lantern_ops",
            "+44 7700 900123",
            0.7,
            "the number posted in a reply",
        ),
        (
            "+44 7700 900123",
            "relay@example.net",
            0.9,
            "both in one WHOIS record",
        ),
        (
            "lantern@example.org",
            "relay@example.net",
            0.4,
            "a guess on a forum",
        ),
        (
            "relay@example.net",
            "@moth_relay",
            0.85,
            "a password-reset page shows r****@example.net",
        ),
    ];
    println!("  evidence, with how sure each link is:");
    for (a, b, confidence, why) in evidence {
        println!("    {confidence:.2}  {a} ── {b}   ({why})");
    }
    let links: Vec<(&str, &str, f64)> = evidence.iter().map(|e| (e.0, e.1, e.2)).collect();
    let chain = osint::strongest_chain(&links, &"lantern@example.org", &"@moth_relay")?
        .ok_or("no chain between them")?;
    line(
        "osint::strongest_chain(evidence, lantern@example.org, @moth_relay)",
        format!(
            "{}   ({} links, {:.2} together)",
            chain.nodes.join(" → "),
            chain.links.len(),
            chain.confidence
        ),
    );
    let shortcut = evidence[3].2 * evidence[4].2;
    println!(
        "      → it beats the two-link route through the forum guess ({shortcut:.2}). It rests on: {}",
        join(chain.links.iter().map(|&i| evidence[i].3), "; ")
    );
    Ok(())
}

fn growth() -> Outcome {
    heading("Complexity", "How the work grows as the data grows.");
    println!("            items    looks for binary search    pairs to check one by one");
    for n in [1_000u64, 1_000_000, 1_000_000_000] {
        println!(
            "    {n:>13}    {:>22}    {:>25}",
            complexity::binary_search_steps(n),
            n * (n - 1) / 2
        );
    }
    println!();
    let timings = [
        (1_000.0, 2.1),
        (2_000.0, 4.0),
        (4_000.0, 8.3),
        (8_000.0, 15.9),
    ];
    let exponent = complexity::growth_exponent(&timings).ok_or("not enough timings")?;
    line(
        "complexity::growth_exponent(four timings that double as N doubles)",
        format!("{exponent:.2}   (about 1 means the work grows in step with N)"),
    );
    println!("  For a real race between O(N) and O(log N): cargo run --release --example growth");
    Ok(())
}
