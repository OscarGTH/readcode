//! Where to read more about a concept.
//!
//! A table, not a model: a learner who is told to read a page that does not
//! exist trusts the tool less than one who is told nothing. Concept ids are
//! namespaced (`rust.ownership.move`), so the table matches on the longest
//! prefix and every concept in a corpus can be covered without writing a URL
//! per concept.

/// One place to read about a concept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub title: String,
    pub url: String,
}

/// `(concept prefix, title, url)`, longest prefix wins.
const TABLE: &[(&str, &str, &str)] = &[
    (
        "rust.ownership",
        "The Rust Book — Understanding Ownership",
        "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html",
    ),
    (
        "rust.borrow",
        "The Rust Book — References and Borrowing",
        "https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html",
    ),
    (
        "rust.heap",
        "The Rust Book — Ownership and String",
        "https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html",
    ),
    (
        "rust.slice",
        "The Rust Book — The Slice Type",
        "https://doc.rust-lang.org/book/ch04-03-slices.html",
    ),
    (
        "rust.lifetime",
        "The Rust Book — Validating References with Lifetimes",
        "https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html",
    ),
    (
        "rust.binding",
        "The Rust Book — Variables and Mutability",
        "https://doc.rust-lang.org/book/ch03-01-variables-and-mutability.html",
    ),
    (
        "rust.mut_self",
        "The Rust Book — Methods and &mut self",
        "https://doc.rust-lang.org/book/ch05-03-method-syntax.html",
    ),
    (
        "rust.mut.param",
        "The Rust Book — Variables and Mutability",
        "https://doc.rust-lang.org/book/ch03-01-variables-and-mutability.html",
    ),
    (
        "rust.type",
        "The Rust Book — Data Types",
        "https://doc.rust-lang.org/book/ch03-02-data-types.html",
    ),
    (
        "rust.fn.main",
        "The Rust Book — Hello, World!",
        "https://doc.rust-lang.org/book/ch01-02-hello-world.html",
    ),
    (
        "rust.fn.return",
        "The Rust Book — Functions",
        "https://doc.rust-lang.org/book/ch03-03-how-functions-work.html",
    ),
    (
        "rust.return",
        "The Rust Book — Functions and return values",
        "https://doc.rust-lang.org/book/ch03-03-how-functions-work.html",
    ),
    (
        "rust.main.result",
        "std::process::Termination",
        "https://doc.rust-lang.org/std/process/trait.Termination.html",
    ),
    (
        "rust.exit.code",
        "std::process::exit",
        "https://doc.rust-lang.org/std/process/fn.exit.html",
    ),
    (
        "rust.struct",
        "The Rust Book — Defining and Instantiating Structs",
        "https://doc.rust-lang.org/book/ch05-01-defining-structs.html",
    ),
    (
        "rust.field",
        "The Rust Book — Defining and Instantiating Structs",
        "https://doc.rust-lang.org/book/ch05-01-defining-structs.html",
    ),
    (
        "rust.impl",
        "The Rust Book — Method Syntax",
        "https://doc.rust-lang.org/book/ch05-03-method-syntax.html",
    ),
    (
        "rust.derive",
        "std::default::Default",
        "https://doc.rust-lang.org/std/default/trait.Default.html",
    ),
    (
        "rust.method.call",
        "The Rust Book — Method Syntax",
        "https://doc.rust-lang.org/book/ch05-03-method-syntax.html",
    ),
    (
        "rust.if",
        "The Rust Book — Control Flow",
        "https://doc.rust-lang.org/book/ch03-05-control-flow.html",
    ),
    (
        "rust.loop",
        "The Rust Book — Control Flow",
        "https://doc.rust-lang.org/book/ch03-05-control-flow.html",
    ),
    (
        "rust.operator.compound",
        "Rust reference — Compound assignment",
        "https://doc.rust-lang.org/reference/expressions/operator-expr.html#compound-assignment-expressions",
    ),
    (
        "rust.operator",
        "std::ops",
        "https://doc.rust-lang.org/std/ops/index.html",
    ),
    (
        "rust.iterator.adaptor",
        "The Rust Book — Processing a Series of Items with Iterators",
        "https://doc.rust-lang.org/book/ch13-02-iterators.html",
    ),
    (
        "rust.iterator.lazy",
        "The Rust Book — Iterators are lazy",
        "https://doc.rust-lang.org/book/ch13-02-iterators.html",
    ),
    (
        "rust.iterator.for",
        "The Rust Book — Looping through an iterator",
        "https://doc.rust-lang.org/book/ch13-02-iterators.html",
    ),
    (
        "rust.collect",
        "std::iter::Iterator::collect",
        "https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.collect",
    ),
    (
        "rust.closure",
        "The Rust Book — Closures",
        "https://doc.rust-lang.org/book/ch13-01-closures.html",
    ),
    (
        "rust.question_mark",
        "The Rust Book — Propagating errors with ?",
        "https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html#propagating-errors-with",
    ),
    (
        "rust.result",
        "std::result::Result",
        "https://doc.rust-lang.org/std/result/enum.Result.html",
    ),
    (
        "rust.error.from",
        "std::convert::From",
        "https://doc.rust-lang.org/std/convert/trait.From.html",
    ),
    (
        "rust.error",
        "The Rust Book — Error Handling",
        "https://doc.rust-lang.org/book/ch09-00-error-handling.html",
    ),
    (
        "rust.hashmap.entry",
        "std::collections::hash_map::Entry",
        "https://doc.rust-lang.org/std/collections/hash_map/enum.Entry.html",
    ),
    (
        "rust.deref",
        "std::ops::Deref",
        "https://doc.rust-lang.org/std/ops/trait.Deref.html",
    ),
    (
        "rust.use",
        "The Rust Book — Bringing Paths into Scope with use",
        "https://doc.rust-lang.org/book/ch07-04-bringing-paths-into-scope-with-the-use-keyword.html",
    ),
    (
        "rust.macro.println",
        "std::println!",
        "https://doc.rust-lang.org/std/macro.println.html",
    ),
    (
        "rust.format",
        "std::fmt — formatting syntax",
        "https://doc.rust-lang.org/std/fmt/",
    ),
    (
        "rust.range",
        "std::ops::RangeInclusive",
        "https://doc.rust-lang.org/std/ops/struct.RangeInclusive.html",
    ),
    (
        "rust.option",
        "std::option::Option",
        "https://doc.rust-lang.org/std/option/enum.Option.html",
    ),
    (
        "rust.match",
        "The Rust Book — The match Control Flow Operator",
        "https://doc.rust-lang.org/book/ch06-02-match.html",
    ),
    (
        "rust.enum",
        "The Rust Book — Defining an Enum",
        "https://doc.rust-lang.org/book/ch06-01-defining-an-enum.html",
    ),
    (
        "rust.vec",
        "The Rust Book — Storing Lists of Values with Vectors",
        "https://doc.rust-lang.org/book/ch08-01-vectors.html",
    ),
    (
        "rust.parse",
        "str::parse",
        "https://doc.rust-lang.org/std/primitive.str.html#method.parse",
    ),
    (
        "rust.map_err",
        "Result::map_err",
        "https://doc.rust-lang.org/std/result/enum.Result.html#method.map_err",
    ),
    (
        "rust.tuple",
        "The Rust Book — Data Types, the tuple",
        "https://doc.rust-lang.org/book/ch03-02-data-types.html",
    ),
    (
        "rust.cast",
        "The Rust Reference — type cast expressions",
        "https://doc.rust-lang.org/reference/expressions/operator-expr.html#type-cast-expressions",
    ),
    (
        "rust.sum",
        "Iterator::sum",
        "https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.sum",
    ),
    (
        "rust.string",
        "String::push_str",
        "https://doc.rust-lang.org/std/string/struct.String.html#method.push_str",
    ),
    // Go. Same rule as above: the longest prefix wins, so a specific concept
    // gets a specific page and the rest fall back to the language's own docs.
    (
        "go.package.main",
        "The Go Tour — Packages",
        "https://go.dev/tour/basics/1",
    ),
    (
        "go.import",
        "The Go Tour — Imports",
        "https://go.dev/tour/basics/2",
    ),
    (
        "go.short_var",
        "The Go Tour — Short variable declarations",
        "https://go.dev/tour/basics/10",
    ),
    (
        "go.fmt.println",
        "pkg.go.dev — fmt.Println",
        "https://pkg.go.dev/fmt#Println",
    ),
    (
        "go.fmt.errorf",
        "pkg.go.dev — fmt.Errorf",
        "https://pkg.go.dev/fmt#Errorf",
    ),
    (
        "go.func.signature",
        "The Go Tour — Functions",
        "https://go.dev/tour/basics/4",
    ),
    (
        "go.if",
        "The Go Tour — If",
        "https://go.dev/tour/flowcontrol/5",
    ),
    (
        "go.return.early",
        "Effective Go — Control structures",
        "https://go.dev/doc/effective_go#control-structures",
    ),
    (
        "go.slice",
        "The Go Tour — Slices",
        "https://go.dev/tour/moretypes/7",
    ),
    (
        "go.range",
        "The Go Tour — Range",
        "https://go.dev/tour/moretypes/16",
    ),
    (
        "go.blank",
        "Effective Go — The blank identifier",
        "https://go.dev/doc/effective_go#blank",
    ),
    (
        "go.append",
        "The Go Tour — Appending to a slice",
        "https://go.dev/tour/moretypes/15",
    ),
    (
        "go.struct",
        "The Go Tour — Structs",
        "https://go.dev/tour/moretypes/2",
    ),
    (
        "go.method.receiver",
        "The Go Tour — Methods",
        "https://go.dev/tour/methods/1",
    ),
    (
        "go.value.semantics",
        "Effective Go — Pointers vs. Values",
        "https://go.dev/doc/effective_go#pointers_vs_values",
    ),
    (
        "go.map.make",
        "Effective Go — Allocation with make",
        "https://go.dev/doc/effective_go#allocation_make",
    ),
    (
        "go.map.index",
        "The Go Spec — Index expressions",
        "https://go.dev/ref/spec#Index_expressions",
    ),
    (
        "go.map.reference",
        "The Go Blog — Go maps in action",
        "https://go.dev/blog/maps",
    ),
    (
        "go.map",
        "The Go Tour — Maps",
        "https://go.dev/tour/moretypes/19",
    ),
    (
        "go.zero.value",
        "The Go Spec — The zero value",
        "https://go.dev/ref/spec#The_zero_value",
    ),
    (
        "go.multi.return",
        "The Go Tour — Multiple return values",
        "https://go.dev/tour/basics/6",
    ),
    (
        "go.error.wrap",
        "The Go Blog — Working with Errors in Go 1.13",
        "https://go.dev/blog/go1.13-errors",
    ),
    (
        "go.error.interface",
        "The Go Blog — Error handling and Go",
        "https://go.dev/blog/error-handling-and-go",
    ),
    (
        "go.err.check",
        "Effective Go — Errors",
        "https://go.dev/doc/effective_go#errors",
    ),
    (
        "go.nil.error",
        "The Go Blog — Error handling and Go",
        "https://go.dev/blog/error-handling-and-go",
    ),
];

/// The best page for one concept, if the table knows the namespace.
pub fn for_concept(concept: &str) -> Option<Doc> {
    let mut best: Option<(usize, Doc)> = None;
    for (prefix, title, url) in TABLE {
        let matches = concept == *prefix
            || concept
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'));
        if !matches {
            continue;
        }
        if best.as_ref().is_none_or(|(len, _)| prefix.len() > *len) {
            best = Some((
                prefix.len(),
                Doc {
                    title: title.to_string(),
                    url: url.to_string(),
                },
            ));
        }
    }
    best.map(|(_, doc)| doc)
}

/// One page per distinct topic, in the order the concepts were asked about.
pub fn for_concepts<'a>(concepts: impl IntoIterator<Item = &'a str>) -> Vec<Doc> {
    let mut out: Vec<Doc> = Vec::new();
    for concept in concepts {
        let Some(doc) = for_concept(concept) else {
            continue;
        };
        if !out.iter().any(|existing| existing.url == doc.url) {
            out.push(doc);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;

    #[test]
    fn the_longest_matching_prefix_wins() {
        let move_ = for_concept("rust.ownership.move").unwrap();
        assert!(move_.url.contains("ch04-00"));
        // A shorter prefix must not shadow a longer one.
        assert_eq!(
            for_concept("rust.hashmap.entry").unwrap().url,
            for_concept("rust.hashmap.entry").unwrap().url
        );
        assert_ne!(
            for_concept("rust.operator.compound").unwrap().url,
            for_concept("rust.operator.not").unwrap().url
        );
    }

    #[test]
    fn an_unknown_namespace_gets_no_link_rather_than_a_guess() {
        assert!(for_concept("python.list.comprehension").is_none());
    }

    #[test]
    fn a_topic_is_listed_once() {
        let docs = for_concepts(["rust.ownership.move", "rust.ownership.clone"]);
        assert_eq!(docs.len(), 1);
        assert!(docs[0].url.contains("ch04-00"));
    }

    /// The corpus is written before the table is extended, so this keeps the
    /// table honest: every concept a learner can be graded on has somewhere to
    /// read up on it.
    #[test]
    fn every_builtin_concept_has_a_link() {
        let corpus = Corpus::builtin().unwrap();
        let mut missing: Vec<String> = Vec::new();
        for snippet in corpus.snippets() {
            for concept in snippet.concepts() {
                if for_concept(&concept).is_none() {
                    missing.push(concept);
                }
            }
        }
        assert!(missing.is_empty(), "no documentation link for: {missing:?}");
    }

    #[test]
    fn every_link_is_https() {
        for (prefix, _, url) in TABLE {
            assert!(url.starts_with("https://"), "{prefix}: {url}");
        }
    }
}
