# SkeleTrace security notes

This is the 1.0.0 standalone release. Report vulnerabilities privately by email to
info@skeletrace.com, not in public issues. There is no promised response schedule.
Include the version, affected component, reproducible steps and likely impact,
using synthetic inputs and redacted credentials.

Profiles and request files are local configuration. Authentication values can be
literal fields in profiles, so protect those files and any exported configuration.
Source adapters contact only the endpoints selected in their configuration. Use
source policy/capability settings appropriate to the service and your authorization.

Snapshot stems must be portable single filenames and cannot specify paths outside
the configured export directory. Export destinations and SQLite files remain local
filesystem resources controlled by the operator. Keep those directories protected.

The crate contains no embedded packet capture, case-notebook database, external
application launcher or automatic background collector. Dependency versions are
pinned in `Cargo.lock`; intentional updates should be tested before distribution.
