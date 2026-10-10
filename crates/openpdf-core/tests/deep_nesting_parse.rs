//! Untrusted input: deeply nested arrays must give `Err`, never a crash
//! (stack overflow aborts the whole process). RUSTSEC-2026-0187.

fn nested_pdf(depth: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let o1 = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Deep ");
    out.extend(std::iter::repeat_n(b'[', depth));
    out.extend(std::iter::repeat_n(b']', depth));
    out.extend_from_slice(b" >>\nendobj\n");
    let o2 = out.len();
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
    let x = out.len();
    out.extend_from_slice(
        format!(
            "xref\n0 3\n0000000000 65535 f \n{o1:010} 00000 n \n{o2:010} 00000 n \ntrailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

fn check(depth: usize) {
    // Run on a normal-sized (2 MiB) thread like the test harness default, so
    // the result does not depend on the main thread's larger stack.
    let r = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || openpdf_core::Document::from_bytes(nested_pdf(depth), None).map(|_| ()))
        .unwrap()
        .join()
        .expect("from_bytes panicked");
    assert!(
        r.is_err(),
        "depth {depth}: nested arrays accepted as a valid document"
    );
}

#[test]
#[ignore = "aborts with stack overflow on lopdf 0.36 (RUSTSEC-2026-0187); un-ignore when #35 (lopdf 0.44) lands"]
fn nested_arrays_10k_is_err() {
    check(10_000);
}

#[test]
#[ignore = "aborts with stack overflow on lopdf 0.36 (RUSTSEC-2026-0187); un-ignore when #35 (lopdf 0.44) lands"]
fn nested_arrays_100k_is_err() {
    check(100_000);
}
