#[global_allocator]
static ALLOCATOR: flux::alloc_count::CountingAlloc = flux::alloc_count::CountingAlloc;

fn main() -> std::process::ExitCode {
    flux::cli::run(std::env::args_os())
}
