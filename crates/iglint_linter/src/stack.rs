//! Stacks for the threads that walk syntax trees.
//!
//! Parsers bound the depth of every tree (`iglint_python::MAX_DEPTH`,
//! `iglint_expression::MAX_DEPTH`, the JSON nesting limit), but the
//! consumers (visitors, the semantic model, summaries, rules) recurse, and
//! a tree at those limits needs more than the default thread stack. Run
//! linting on threads with [`STACK_SIZE`]: [`run_with_stack`] for the
//! calling thread and [`pool_builder`] (or [`configure_global_pool`]) for
//! `rayon`. Only the touched pages of a stack are committed.

/// Stack size of every thread that lints.
pub const STACK_SIZE: usize = 256 * 1024 * 1024;

/// Runs `f` on a thread with a [`STACK_SIZE`] stack (on the calling thread
/// when no thread can be started).
pub fn run_with_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    let mut f = Some(f);
    let result = std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("iglint".to_owned())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, || (f.take().expect("not run"))());
        spawned.map(|handle| handle.join())
    });
    match result {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => (f.take().expect("not run"))(),
    }
}

/// A `rayon` pool builder whose threads have [`STACK_SIZE`] stacks.
pub fn pool_builder() -> rayon::ThreadPoolBuilder {
    rayon::ThreadPoolBuilder::new().stack_size(STACK_SIZE)
}

/// Makes the global `rayon` pool use [`STACK_SIZE`] stacks. Does nothing
/// when the global pool already exists.
pub fn configure_global_pool() {
    let _ = pool_builder().build_global();
}
