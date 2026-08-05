# Reviewed model-catalog metadata

This directory is the only repository source accepted by the protected
model-catalog publisher. A publication request names one tracked canonical JSON
`CatalogDocument` here and its exact SHA-256. The workflow checks both identities
at the reviewed main commit before it assumes AWS credentials.

No catalog document is checked in yet. The first one must be built from real
provider-conformance receipts and reviewed exact provider-native model ids. Do
not copy fixture ids, model-list responses, documentation examples, or guessed
slugs into this directory. Staged entries cannot make `brain-mux` serviceable,
and the publisher refuses a document without at least one receipt-backed
`Active` entry.

Documents are immutable review inputs. A correction or later revision receives
a new file and source commit; it never rewrites the document named by an
existing signed collection.
