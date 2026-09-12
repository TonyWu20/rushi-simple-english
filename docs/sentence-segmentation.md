# Sentence segmentation

## Context

The `run.idle` hook lints the last assistant reply. It splits the text
into sentences in `src/sentences.rs`.

A bug made `sentence_ends` return character indices. The caller sliced the
source with those indices as byte offsets. The slice landed mid-character
on a multibyte line. The process then panicked.

## Decision

`src/sentences.rs` changed in two ways.

1. `sentence_ends` now tracks byte offsets. Every index is a valid char
   boundary.

2. The terminator set now includes the CJK and full-width marks. They sit
   alongside the ASCII ones. The marks are the ideographic full stop, the
   fullwidth exclamation, and the fullwidth question mark.

## Rationale

String slicing uses byte offsets. Mixing character and byte indexes caused
the panic. The fix works in byte offsets end to end.

The CJK terminators were added so CJK text splits into sentences too. They
share the same match arm and the same consuming loop.

## Consequences

- English behaviour is unchanged. The ASCII marks still work the same way.

- CJK text now splits at its own terminators. It no longer collapses into
  one long sentence.

- The regression test `multibyte_chars_before_terminator` guards the
  char-boundary fix.

- The `cjk_sentence` test guards the CJK terminators.

## Commits

- `bfb0076` fixed the panic and added the CJK terminators.

- `84d35ff` is the unrelated prose-coordinate diff pair.
