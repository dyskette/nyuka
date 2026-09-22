//! Per-source concurrency limiting.
//!
//! One `Semaphore` per source, bounding page fetches inside a
//! `download_chapter` handler. Correct only because there is one process: two
//! processes with the same configuration produce twice the request rate toward
//! the site, and the consequence is an IP ban on the sites the application
//! exists to read (ADR-0003).
//!
//! Sources declare their own limit through the `net::set_rate_limit(permits,
//! period, unit)` host import. Take the **stricter** of the declared limit and
//! the configured cap, and log when the source is stricter (ADR-0004).
