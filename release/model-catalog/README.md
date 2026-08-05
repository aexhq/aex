# Reviewed model-catalog compatibility sources

This directory is the only repository source accepted by the protected
model-catalog publisher. Each immutable `*.source.json` records reviewed static
compatibility metadata: the provider-native model identity and the closed
capabilities and limits supported by the current adapter/runtime contract.

The publisher validates the tracked source identity, uses the repository-owned
generator to create the canonical signed document, and independently validates
the source/document pair before obtaining AWS credentials. Provider model-list
responses, live health observations, qualification artifacts, and guessed
model slugs are not publication inputs.

Merging one changed `*.source.json` to `main` starts the protected publication
lane; exact manual dispatch remains available. A correction or revision gets a
new source file and sequence. It never rewrites a source named by an existing
signed collection.

Publication creates an immutable candidate only. It does not update the
repository's `AEX_MODEL_CATALOG_BINDING_JSON`; until that one canonical value is
independently reviewed and atomically replaced, builds keep consuming the
previous last-good signed collection.
