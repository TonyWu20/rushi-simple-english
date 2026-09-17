//! Embedded dictionary data ported from jyooi/agent-simple-english.

/// A set of unapproved words or phrases with suggested replacements.
#[derive(Debug, Clone)]
pub struct PhraseEntry {
    pub unapproved: Vec<String>,
    pub suggestions: Vec<String>,
}

fn entry(unapproved: &[&str], suggestions: &[&str]) -> PhraseEntry {
    PhraseEntry {
        unapproved: unapproved.iter().map(|s| s.to_string()).collect(),
        suggestions: suggestions.iter().map(|s| s.to_string()).collect(),
    }
}

/// Phrasal verbs (11 entries).
pub fn phrasal_verbs() -> Vec<PhraseEntry> {
    vec![
        entry(&["carry out", "carries out", "carried out", "carrying out"], &["do"]),
        entry(&["spin up", "spins up", "spun up", "spinning up"], &["start"]),
        entry(&["spin down", "spins down", "spun down", "spinning down"], &["stop"]),
        entry(&["tear down", "tears down", "tore down", "torn down", "tearing down"], &["remove"]),
        entry(&["reach out", "reaches out", "reached out", "reaching out"], &["ask"]),
        entry(&["dive into", "dives into", "dived into", "dove into", "diving into"], &["examine"]),
        entry(&["kick off", "kicks off", "kicked off", "kicking off"], &["start"]),
        entry(&["roll out", "rolls out", "rolled out", "rolling out"], &["release"]),
        entry(&["ramp up", "ramps up", "ramped up", "ramping up"], &["increase"]),
        entry(&["circle back", "circles back", "circled back", "circling back"], &["return"]),
        entry(&["drill down", "drills down", "drilled down", "drilling down"], &["examine"]),
    ]
}

/// Hedging phrases.
pub fn hedging() -> Vec<PhraseEntry> {
    vec![entry(
        &[
            "it is important to note",
            "it should be noted",
            "it is worth noting",
            "please note that",
            "as mentioned",
            "as noted above",
        ],
        &["delete"],
    )]
}

/// Marketing language.
pub fn marketing() -> Vec<PhraseEntry> {
    vec![entry(
        &[
            "seamless", "seamlessly", "robust", "powerful",
            "cutting-edge", "effortless", "effortlessly",
            "world-class", "next-generation", "revolutionary",
            "blazing", "lightning-fast", "elegant", "delightful",
            "turnkey", "best-in-class", "state-of-the-art",
            "game-changing", "battle-tested", "enterprise-grade",
            "supercharge", "unleash", "empower", "empowers",
        ],
        &["delete"],
    )]
}

/// ASD-STE100 not-approved word list.
pub fn ste_dictionary() -> Vec<PhraseEntry> {
    vec![
        entry(&["initiate", "initiates", "initiated", "initiating"], &["start"]),
        entry(&["commence", "commences", "commenced", "commencing"], &["start"]),
        entry(&["utilize", "utilizes", "utilized", "utilizing"], &["use"]),
        entry(&["utilise", "utilises", "utilised", "utilising"], &["use"]),
        entry(&["ensure", "ensures", "ensured", "ensuring"], &["make sure"]),
        entry(&["terminate", "terminates", "terminated", "terminating"], &["stop"]),
        entry(&["facilitate", "facilitates", "facilitated", "facilitating"], &["help"]),
        entry(&["locate", "locates", "located", "locating"], &["find"]),
        entry(&["indicate", "indicates", "indicated", "indicating"], &["show"]),
        entry(&["require", "requires", "required", "requiring"], &["need"]),
        entry(&["purchase", "purchases", "purchased", "purchasing"], &["buy"]),
        entry(&["leverage", "leverages", "leveraged", "leveraging"], &["use"]),
        entry(&["acquire", "acquires", "acquired", "acquiring"], &["get"]),
        entry(&["demonstrate", "demonstrates", "demonstrated", "demonstrating"], &["show"]),
        entry(&["originate", "originates", "originated", "originating"], &["start"]),
        entry(&["perform", "performs", "performed", "performing"], &["do"]),
        entry(&["obtain", "obtains", "obtained", "obtaining"], &["get"]),
        entry(&["attempt", "attempts", "attempted", "attempting"], &["try"]),
        entry(&["assist", "assists", "assisted", "assisting"], &["help"]),
        entry(&["permit", "permits", "permited", "permiting"], &["let"]),
        entry(&["modify", "modifies", "modified", "modifying"], &["change"]),
        entry(&["begin", "begins", "began", "beginning"], &["start"]),
        entry(&["approximately"], &["about"]),
        entry(&["sufficient"], &["enough"]),
        entry(&["subsequent", "subsequently"], &["next"]),
        entry(&["prior"], &["before"]),
        entry(&["additional", "additionally"], &["more"]),
        entry(&["furthermore", "moreover"], &["also"]),
        entry(&["comprehensive", "comprehensively"], &["complete"]),
        entry(&["utilization"], &["use"]),
        entry(&["aforementioned"], &["this"]),
        entry(&["whilst"], &["while"]),
        entry(&["amongst"], &["among"]),
        entry(&["numerous", "myriad", "plethora"], &["many"]),
        entry(&["prior to"], &["before"]),
        entry(&["subsequent to"], &["after"]),
        entry(&["in order to"], &["to"]),
        entry(&["a variety of"], &["some"]),
        entry(&["in the event that"], &["if"]),
        entry(&["due to the fact that"], &["because"]),
        entry(&["is able to", "are able to"], &["can"]),
        entry(&["make use of", "makes use of"], &["use"]),
        entry(&["provenance"], &["origin"]),
    ]
}
