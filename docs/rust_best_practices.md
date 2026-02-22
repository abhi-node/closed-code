# Rust Best Practices Guide

A comprehensive, research-backed guide to writing idiomatic, maintainable, and performant Rust code.

---

## Table of Contents

1. [Project Structure & Organization](#1-project-structure--organization)
2. [Naming Conventions](#2-naming-conventions)
3. [Ownership, Borrowing & Lifetimes](#3-ownership-borrowing--lifetimes)
4. [Error Handling](#4-error-handling)
5. [Type System & Data Modeling](#5-type-system--data-modeling)
6. [Traits & Generics](#6-traits--generics)
7. [Iterators & Functional Patterns](#7-iterators--functional-patterns)
8. [Concurrency & Async](#8-concurrency--async)
9. [Testing](#9-testing)
10. [Performance](#10-performance)
11. [Tooling & Linting](#11-tooling--linting)
12. [API Design](#12-api-design)
13. [Dependency Management](#13-dependency-management)
14. [Security](#14-security)
15. [Common Anti-Patterns](#15-common-anti-patterns)

---

## 1. Project Structure & Organization

### Basic Layout

```
my_project/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs          # Binary entry point
│   ├── lib.rs           # Library root (public API)
│   ├── config.rs        # Module file
│   ├── models/
│   │   ├── mod.rs       # Module declaration
│   │   ├── user.rs
│   │   └── product.rs
│   └── services/
│       ├── mod.rs
│       ├── auth.rs
│       └── database.rs
├── tests/               # Integration tests
│   └── integration_test.rs
├── benches/             # Benchmarks
│   └── my_benchmark.rs
├── examples/            # Example programs
│   └── basic_usage.rs
└── README.md
```

### Module Organization

Keep modules focused on a single responsibility. Split code into modules when a file exceeds ~300–400 lines or when distinct concerns emerge.

```rust
// src/lib.rs — Declare and re-export your public API
pub mod config;
pub mod models;
pub mod services;

// Re-export key types for ergonomic access
pub use config::AppConfig;
pub use models::User;
```

```rust
// src/models/mod.rs — Group related types
mod user;
mod product;

pub use user::User;
pub use product::Product;
```

### Workspace Layout for Larger Projects

When a project grows into multiple crates, use a Cargo workspace:

```toml
# Cargo.toml (workspace root)
[workspace]
members = [
    "crates/core",
    "crates/api",
    "crates/cli",
]
```

This lets you share dependencies, run tests across the entire project, and enforce a clear boundary between components.

---

## 2. Naming Conventions

Rust enforces naming conventions through compiler warnings. Follow them consistently:

| Item                    | Convention            | Example                  |
|-------------------------|-----------------------|--------------------------|
| Variables, functions    | `snake_case`          | `let user_name = ...;`   |
| Types, traits, enums    | `UpperCamelCase`      | `struct HttpClient`       |
| Enum variants           | `UpperCamelCase`      | `Color::DarkRed`          |
| Constants, statics      | `SCREAMING_SNAKE_CASE`| `const MAX_RETRIES: u32`  |
| Modules, crate names    | `snake_case`          | `mod user_auth;`          |
| Type parameters         | Short `UpperCamelCase`| `<T>`, `<K, V>`, `<E>`   |
| Lifetimes               | Short lowercase       | `'a`, `'de`, `'src`      |

### Naming Tips

```rust
// ✅ Good: descriptive and idiomatic
fn calculate_total_price(items: &[Item]) -> Decimal { ... }

// ❌ Bad: abbreviated and unclear
fn calc_tp(i: &[Item]) -> Decimal { ... }

// ✅ Good: conversion methods follow the as_/to_/into_ convention
impl Temperature {
    fn as_celsius(&self) -> f64 { ... }      // cheap, borrowed view
    fn to_fahrenheit(&self) -> Temperature { ... }  // potentially expensive, new value
    fn into_kelvin(self) -> Kelvin { ... }    // consumes self
}

// ✅ Good: builder/constructor naming
impl Server {
    fn new(port: u16) -> Self { ... }          // standard constructor
    fn builder() -> ServerBuilder { ... }      // builder pattern entry point
}

// ✅ Good: boolean methods read like predicates
fn is_empty(&self) -> bool { ... }
fn has_permission(&self, perm: Permission) -> bool { ... }
```

---

## 3. Ownership, Borrowing & Lifetimes

### Prefer Borrowing Over Cloning

```rust
// ✅ Good: borrow when you only need to read
fn greet(name: &str) {
    println!("Hello, {name}!");
}

// ❌ Unnecessary clone
fn greet_bad(name: String) {
    println!("Hello, {name}!");
}
```

### Accept Generics for Flexible Inputs

```rust
// ✅ Accepts &str, String, &String, Cow<str>, etc.
fn set_name(name: impl Into<String>) {
    let name: String = name.into();
    // ...
}
```

### Prefer Immutability by Default

```rust
let x = 10;          // immutable — prefer this
let mut y = 20;      // mutable only when needed

// ✅ Use shadowing instead of mutability for transformations
let data = read_raw_data();
let data = parse(data);
let data = validate(data);
```

### Minimize Lifetime Annotations

Rely on lifetime elision rules when possible. Only annotate lifetimes when the compiler requires it.

```rust
// Elision handles this — no annotations needed
fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

// Explicit lifetimes only when truly needed (multiple references)
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() { x } else { y }
}
```

### Use `Cow<str>` for Flexible Ownership

```rust
use std::borrow::Cow;

fn normalize_name(name: &str) -> Cow<str> {
    if name.contains(' ') {
        Cow::Owned(name.replace(' ', "_"))
    } else {
        Cow::Borrowed(name) // no allocation
    }
}
```

---

## 4. Error Handling

### Use `Result` and `Option`, Never Panic in Libraries

```rust
// ✅ Good: return a Result
fn parse_port(s: &str) -> Result<u16, ParseError> {
    s.parse::<u16>().map_err(ParseError::InvalidPort)
}

// ❌ Bad in library code: panics on invalid input
fn parse_port_bad(s: &str) -> u16 {
    s.parse().unwrap() // panics!
}
```

### Use `thiserror` for Libraries

Define structured, matchable error types that callers can inspect:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("connection failed: {0}")]
    ConnectionFailed(#[from] std::io::Error),

    #[error("query failed: {query}")]
    QueryFailed { query: String, source: sqlx::Error },

    #[error("record not found: {0}")]
    NotFound(String),
}
```

### Use `anyhow` for Applications

In binaries and application code, use `anyhow` for ergonomic error propagation:

```rust
use anyhow::{Context, Result};

fn load_config(path: &str) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config from {path}"))?;

    let config: Config = toml::from_str(&contents)
        .context("failed to parse config file")?;

    Ok(config)
}

fn main() -> Result<()> {
    let config = load_config("config.toml")?;
    run_server(config)?;
    Ok(())
}
```

### Use the `?` Operator Consistently

```rust
// ✅ Clean error propagation
fn fetch_user_email(id: u64) -> Result<String> {
    let user = db::find_user(id).context("user lookup failed")?;
    let profile = db::find_profile(user.profile_id).context("profile lookup failed")?;
    Ok(profile.email)
}

// ❌ Avoid deeply nested match chains
fn fetch_user_email_bad(id: u64) -> Result<String> {
    match db::find_user(id) {
        Ok(user) => match db::find_profile(user.profile_id) {
            Ok(profile) => Ok(profile.email),
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(e.into()),
    }
}
```

### When It's OK to `unwrap()`

Reserve `unwrap()` and `expect()` for situations where failure is logically impossible:

```rust
// ✅ OK: regex is a compile-time constant
let re = Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("hardcoded regex is valid");

// ✅ OK: in tests
#[test]
fn test_parsing() {
    let result = parse("42").unwrap();
    assert_eq!(result, 42);
}
```

---

## 5. Type System & Data Modeling

### Use Newtypes to Prevent Mixing Up Values

```rust
struct UserId(u64);
struct OrderId(u64);

// Now the compiler prevents you from accidentally passing
// a UserId where an OrderId is expected
fn get_order(order_id: OrderId) -> Option<Order> { ... }
```

### Use Enums for State Machines

```rust
enum ConnectionState {
    Disconnected,
    Connecting { attempt: u32 },
    Connected { session_id: String },
    Error { message: String, retryable: bool },
}

impl ConnectionState {
    fn is_active(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}
```

### Make Invalid States Unrepresentable

```rust
// ❌ Bad: allows invalid combinations
struct Ticket {
    status: String,              // "open", "closed", ...?
    assigned_to: Option<String>, // what if closed but assigned?
    resolution: Option<String>,  // what if open but has resolution?
}

// ✅ Good: the type system enforces valid states
enum Ticket {
    Open { title: String, description: String },
    InProgress { title: String, assignee: String },
    Resolved { title: String, assignee: String, resolution: String },
}
```

### Prefer Struct Variants for Complex Data

```rust
// ✅ Named fields are self-documenting
enum Command {
    Move { x: f64, y: f64, speed: f64 },
    Rotate { degrees: f64 },
    Stop,
}

// ❌ Tuple variants are opaque when they have more than 1–2 fields
enum Command {
    Move(f64, f64, f64),  // what is each field?
}
```

---

## 6. Traits & Generics

### Derive Common Traits

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Config {
    pub host: String,
    pub port: u16,
}

// Derive serde traits for serialization
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiResponse<T> {
    pub data: T,
    pub status: u16,
}
```

### Use Trait Bounds Wisely

```rust
// ✅ Use impl Trait for simple cases
fn print_all(items: &[impl Display]) {
    for item in items {
        println!("{item}");
    }
}

// ✅ Use where clauses when bounds get complex
fn process<T, U>(input: T, output: U) -> Result<()>
where
    T: Read + Send + 'static,
    U: Write + Send + 'static,
{
    // ...
}
```

### Use the Builder Pattern for Complex Construction

```rust
pub struct ServerConfig {
    host: String,
    port: u16,
    max_connections: usize,
    timeout: Duration,
}

pub struct ServerConfigBuilder {
    host: String,
    port: u16,
    max_connections: usize,
    timeout: Duration,
}

impl ServerConfigBuilder {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            max_connections: 100,    // sensible default
            timeout: Duration::from_secs(30),
        }
    }

    pub fn max_connections(mut self, n: usize) -> Self {
        self.max_connections = n;
        self
    }

    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = duration;
        self
    }

    pub fn build(self) -> ServerConfig {
        ServerConfig {
            host: self.host,
            port: self.port,
            max_connections: self.max_connections,
            timeout: self.timeout,
        }
    }
}

// Usage:
let config = ServerConfigBuilder::new("0.0.0.0", 8080)
    .max_connections(500)
    .timeout(Duration::from_secs(60))
    .build();
```

### Implement `From` / `Into` for Conversions

```rust
impl From<DatabaseRow> for User {
    fn from(row: DatabaseRow) -> Self {
        User {
            id: row.get("id"),
            name: row.get("name"),
            email: row.get("email"),
        }
    }
}

// Now you can use .into() ergonomically
let user: User = row.into();
```

---

## 7. Iterators & Functional Patterns

### Prefer Iterator Chains Over Manual Loops

```rust
// ✅ Idiomatic: iterator chain
let active_emails: Vec<String> = users
    .iter()
    .filter(|u| u.is_active)
    .map(|u| u.email.clone())
    .collect();

// ❌ Less idiomatic: manual loop with push
let mut active_emails = Vec::new();
for user in &users {
    if user.is_active {
        active_emails.push(user.email.clone());
    }
}
```

### Use `collect()` with Type Annotations

```rust
// Collect into a HashMap
let lookup: HashMap<u64, &User> = users
    .iter()
    .map(|u| (u.id, u))
    .collect();

// Collect Results — short-circuits on first error
let parsed: Result<Vec<u64>, _> = strings
    .iter()
    .map(|s| s.parse::<u64>())
    .collect();
```

### Use `Option` and `Result` Combinators

```rust
// ✅ Chained combinators
fn get_username(id: u64) -> Option<String> {
    find_user(id)
        .filter(|u| u.is_active)
        .map(|u| u.username.clone())
}

// ✅ Provide defaults
let port = env::var("PORT")
    .ok()
    .and_then(|p| p.parse().ok())
    .unwrap_or(8080);
```

---

## 8. Concurrency & Async

### Choose the Right Concurrency Model

- **Threads**: best for CPU-bound work, simpler mental model.
- **Async/await**: best for I/O-bound work with many concurrent operations (e.g., web servers, network clients).
- **Channels**: for communication between tasks or threads.

### Async Best Practices

```rust
use tokio::time::{sleep, Duration};

// ✅ Use tokio::time::sleep, not std::thread::sleep
async fn handle_request(id: u64) -> Result<Response> {
    let user = db::find_user(id).await?;
    let profile = db::find_profile(user.profile_id).await?;
    Ok(build_response(user, profile))
}

// ✅ Use spawn_blocking for CPU-heavy work inside async
async fn process_image(data: Vec<u8>) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        cpu_intensive_resize(&data)
    }).await?
}
```

### Avoid Common Async Pitfalls

```rust
// ❌ Blocking the async runtime
async fn bad_handler() {
    std::thread::sleep(Duration::from_secs(5)); // blocks the executor!
}

// ✅ Use the async-aware equivalent
async fn good_handler() {
    tokio::time::sleep(Duration::from_secs(5)).await;
}

// ❌ Unnecessary spawning for lightweight work
async fn over_spawned() {
    let handle = tokio::spawn(async { 2 + 2 });
    let result = handle.await.unwrap();
}

// ✅ Just .await directly for cheap operations
async fn direct() {
    let result = cheap_async_call().await;
}
```

### Shared State in Async Code

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

// Prefer RwLock over Mutex when reads are frequent
let shared_state = Arc::new(RwLock::new(AppState::default()));

// In handler:
let state = shared_state.read().await;
println!("Current count: {}", state.request_count);
drop(state); // release read lock before acquiring write lock

let mut state = shared_state.write().await;
state.request_count += 1;
```

### Use Channels for Task Communication

```rust
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel::<Event>(100); // bounded channel

// Producer
tokio::spawn(async move {
    tx.send(Event::UserLoggedIn { user_id: 42 }).await.unwrap();
});

// Consumer
while let Some(event) = rx.recv().await {
    process_event(event).await;
}
```

---

## 9. Testing

### Unit Tests (In-Module)

```rust
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_positive() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn test_add_negative() {
        assert_eq!(add(-1, -1), -2);
    }

    #[test]
    #[should_panic(expected = "overflow")]
    fn test_overflow() {
        add(i32::MAX, 1); // should panic in debug mode
    }
}
```

### Integration Tests

```rust
// tests/api_test.rs — runs as a separate crate
use my_project::AppConfig;

#[test]
fn test_config_loading() {
    let config = AppConfig::from_file("tests/fixtures/config.toml").unwrap();
    assert_eq!(config.port, 8080);
}
```

### Async Tests

```rust
#[tokio::test]
async fn test_fetch_user() {
    let pool = setup_test_db().await;
    let user = db::find_user(&pool, 1).await.unwrap();
    assert_eq!(user.name, "Alice");
}
```

### Test Organization Tips

- Place unit tests in the same file as the code they test, inside `#[cfg(test)]` modules.
- Place integration tests in the `tests/` directory.
- Use `tests/fixtures/` for test data files.
- Use test helper functions to reduce duplication, but keep tests readable.

### Property-Based Testing with `proptest`

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_roundtrip_serialization(input in "\\PC*") {
        let encoded = encode(&input);
        let decoded = decode(&encoded).unwrap();
        prop_assert_eq!(input, decoded);
    }
}
```

---

## 10. Performance

### Avoid Unnecessary Allocations

```rust
// ✅ Accept &str instead of String where possible
fn process(input: &str) { ... }

// ✅ Pre-allocate when the size is known
let mut results = Vec::with_capacity(items.len());
for item in items {
    results.push(transform(item));
}

// ✅ Reuse buffers
let mut buf = String::new();
for line in reader.lines() {
    buf.clear();
    buf.push_str(&line?);
    process(&buf);
}
```

### Use `&[T]` Over `&Vec<T>`

```rust
// ✅ Accepts Vec, arrays, slices — more flexible
fn sum(numbers: &[i32]) -> i32 {
    numbers.iter().sum()
}

// ❌ Unnecessarily restrictive
fn sum_bad(numbers: &Vec<i32>) -> i32 {
    numbers.iter().sum()
}
```

### Use `Box<str>` and `Arc<str>` for Immutable Strings

```rust
// When you have a String that will never change, shrink it
let name: Box<str> = some_string.into_boxed_str();

// For shared ownership of immutable strings
let shared: Arc<str> = Arc::from("shared data");
```

### Enable Overflow Checks in Release Builds

```toml
# Cargo.toml
[profile.release]
overflow-checks = true   # catch integer overflow in production
```

### Benchmark Before Optimizing

```rust
// benches/my_benchmark.rs (using criterion)
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_sort(c: &mut Criterion) {
    let mut data: Vec<u64> = (0..10_000).rev().collect();
    c.bench_function("sort 10k", |b| {
        b.iter(|| {
            let mut d = data.clone();
            d.sort();
            black_box(d);
        })
    });
}

criterion_group!(benches, benchmark_sort);
criterion_main!(benches);
```

---

## 11. Tooling & Linting

### Essential Cargo Commands

```bash
cargo fmt              # Format code (enforces Rust style guide)
cargo clippy           # Run the linter (800+ lints)
cargo test             # Run all tests
cargo doc --open       # Generate and open documentation
cargo audit            # Check for known vulnerabilities in dependencies
cargo outdated         # List outdated dependencies
```

### Recommended Clippy Configuration

```toml
# clippy.toml (or .clippy.toml)
too-many-arguments-threshold = 7
type-complexity-threshold = 300
```

```rust
// At the crate root, enable extra lint groups
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]

// Cherry-pick restriction lints you want
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::todo)]
```

### CI Pipeline Setup

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

env:
  RUSTFLAGS: "-Dwarnings"

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --check
      - run: cargo clippy --all-targets --all-features
      - run: cargo test --all-features
```

### Useful Development Tools

| Tool               | Purpose                          | Install                        |
|---------------------|----------------------------------|--------------------------------|
| `cargo-watch`      | Auto-rebuild on changes          | `cargo install cargo-watch`    |
| `cargo-expand`     | View macro-expanded code         | `cargo install cargo-expand`   |
| `cargo-audit`      | Security vulnerability scanning  | `cargo install cargo-audit`    |
| `cargo-outdated`   | Find outdated dependencies       | `cargo install cargo-outdated` |
| `cargo-deny`       | Lint dependencies (licenses, bans) | `cargo install cargo-deny`  |
| `cargo-nextest`    | Faster test runner               | `cargo install cargo-nextest`  |

---

## 12. API Design

### Make APIs Hard to Misuse

```rust
// ✅ Use the type system to guide correct usage
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, EmailError> {
        if raw.contains('@') && raw.contains('.') {
            Ok(Email(raw.to_string()))
        } else {
            Err(EmailError::Invalid(raw.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Now functions can require a validated Email
fn send_email(to: &Email, body: &str) -> Result<()> { ... }
```

### Follow the Principle of Least Surprise

```rust
// ✅ Implement standard traits so your types work as expected
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}
```

### Write Doc Comments with Examples

```rust
/// Splits a string into words, trimming whitespace.
///
/// # Examples
///
/// ```
/// use my_crate::split_words;
///
/// let words = split_words("  hello   world  ");
/// assert_eq!(words, vec!["hello", "world"]);
/// ```
///
/// # Panics
///
/// This function does not panic.
pub fn split_words(input: &str) -> Vec<&str> {
    input.split_whitespace().collect()
}
```

### Mark Exhaustive Decisions Explicitly

```rust
// #[non_exhaustive] signals that new variants may be added
#[non_exhaustive]
pub enum ApiError {
    NotFound,
    Unauthorized,
    RateLimited,
}
```

---

## 13. Dependency Management

### Keep `Cargo.toml` Clean

```toml
[package]
name = "my_project"
version = "0.1.0"
edition = "2024"               # always use the latest edition
rust-version = "1.82"          # minimum supported Rust version
description = "A brief description of the project"
license = "MIT OR Apache-2.0"

[dependencies]
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
proptest = "1"
```

### Dependency Guidelines

- Pin major versions (e.g., `serde = "1"`) and let Cargo resolve patch versions.
- Use `cargo update` regularly to pull in security patches.
- Audit dependencies with `cargo audit` and `cargo deny`.
- Minimize your dependency tree — each dependency is a maintenance and security burden.
- Use feature flags to avoid pulling in unnecessary transitive dependencies.

```toml
# Only enable what you need
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
# Instead of:
# tokio = { version = "1", features = ["full"] }
```

---

## 14. Security

### Minimize `unsafe` Code

```rust
// ✅ If you must use unsafe, isolate it and document the invariants
/// # Safety
/// `ptr` must be a valid, aligned pointer to an initialized `T`.
pub unsafe fn read_value<T>(ptr: *const T) -> T {
    ptr.read()
}
```

### Validate All External Input

```rust
pub fn process_request(input: &str) -> Result<Response> {
    // Validate length
    if input.len() > MAX_INPUT_LENGTH {
        return Err(Error::InputTooLong);
    }

    // Validate content
    let sanitized = sanitize(input)?;

    // Use checked arithmetic
    let total = price
        .checked_mul(quantity)
        .ok_or(Error::Overflow)?;

    Ok(build_response(sanitized, total))
}
```

### Use Trusted Cryptography Crates

```rust
// ✅ Use well-audited libraries
use sha2::{Sha256, Digest};
use argon2::{self, Config};

fn hash_password(password: &[u8], salt: &[u8]) -> String {
    let config = Config::default();
    argon2::hash_encoded(password, salt, &config).unwrap()
}
```

---

## 15. Common Anti-Patterns

### ❌ Overusing `.clone()`

```rust
// ❌ Cloning to appease the borrow checker
let data = expensive_data.clone();
process(&data);

// ✅ Restructure to borrow instead
process(&expensive_data);
```

### ❌ Stringly-Typed APIs

```rust
// ❌ Using strings for structured data
fn set_color(color: &str) { ... }  // what's valid? "red"? "#FF0000"?

// ✅ Use an enum
enum Color { Red, Green, Blue, Custom(u8, u8, u8) }
fn set_color(color: Color) { ... }
```

### ❌ Ignoring Results

```rust
// ❌ Silently discarding errors
let _ = fs::remove_file("temp.txt");

// ✅ Handle or explicitly acknowledge
if let Err(e) = fs::remove_file("temp.txt") {
    tracing::warn!("failed to clean up temp file: {e}");
}
```

### ❌ Overly Complex Generics

```rust
// ❌ Abstraction astronautics
fn process<T, U, V, W>(a: T, b: U) -> V
where
    T: Into<U> + Clone + Send + Sync + 'static,
    U: AsRef<str> + Display + Debug,
    V: From<W> + Default,
    W: TryFrom<T>,
{ ... }

// ✅ Start concrete, generalize only when you have 2+ callers that need it
fn process(input: &str) -> Result<Output> { ... }
```

### ❌ Giant `main.rs` Files

Split your application entry point into small, focused calls:

```rust
// ✅ main.rs should be thin
fn main() -> anyhow::Result<()> {
    let config = config::load()?;
    let _guard = logging::init(&config)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(server::run(config))
}
```

---

## Quick Reference Checklist

- [ ] Use `cargo fmt` and `cargo clippy` before every commit
- [ ] Return `Result` from fallible functions — never panic in libraries
- [ ] Prefer borrowing over cloning
- [ ] Keep `unsafe` blocks minimal, isolated, and documented
- [ ] Write doc comments with `///` and include examples
- [ ] Derive `Debug` on all public types
- [ ] Use `thiserror` in libraries, `anyhow` in applications
- [ ] Write tests alongside your code in `#[cfg(test)]` modules
- [ ] Enable overflow checks in release builds
- [ ] Audit dependencies regularly with `cargo audit`
- [ ] Keep modules focused — split when files grow beyond ~300 lines
- [ ] Use enums to make invalid states unrepresentable

---

*Last updated: February 2026. Sources include the official Rust Book, Rust API Guidelines, Clippy documentation, Comprehensive Rust (Google), and community best practices.*
