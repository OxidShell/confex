//! Validates redb's `MultiWriter` concurrency mode across real OS processes.
//!
//! Each child is a re-exec of this test binary with `CONFEX_MP_CHILD=1`; the
//! child branch short-circuits before the parent's assertions and writes its
//! own slice of keys, then exits. The parent waits for every child and
//! verifies the merged store contains the full expected set.

#![cfg(feature = "redb-backend")]

use std::{path::PathBuf, process::Stdio};

use confex_core::{
    redb::{RedbSource, RedbStore},
    source::Source as _,
    value::ConfigValue,
};

const CHILD_ENV: &str = "CONFEX_MP_CHILD";
const APP: &str = "mp_test";

fn run_child() {
    let db = PathBuf::from(std::env::var("CONFEX_MP_DB").expect("CONFEX_MP_DB"));
    let worker: i64 = std::env::var("CONFEX_MP_WORKER")
        .expect("CONFEX_MP_WORKER")
        .parse()
        .expect("worker int");
    let ops: i64 = std::env::var("CONFEX_MP_OPS")
        .expect("CONFEX_MP_OPS")
        .parse()
        .expect("ops int");

    let store = RedbStore::new(&db, APP);
    for j in 0..ops {
        let key = format!("w{worker}_k{j}");
        store
            .set(&key, &ConfigValue::Integer(worker * 1000 + j))
            .expect("child write failed");
    }
}

#[test]
fn multi_process_writes_no_corruption() {
    if std::env::var(CHILD_ENV).is_ok() {
        run_child();
        return;
    }

    const WORKERS: i64 = 4;
    const OPS: i64 = 25;

    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("mp.redb");

    // Seed the file so children open an initialized DB.
    RedbStore::new(&db, APP)
        .set("seed", &ConfigValue::Bool(true))
        .expect("seed write");

    let exe = std::env::current_exe().expect("current_exe");

    let mut children = Vec::new();
    for w in 0..WORKERS {
        let child = std::process::Command::new(&exe)
            .env(CHILD_ENV, "1")
            .env("CONFEX_MP_DB", &db)
            .env("CONFEX_MP_WORKER", w.to_string())
            .env("CONFEX_MP_OPS", OPS.to_string())
            .args([
                "--exact",
                "multi_process_writes_no_corruption",
                "--nocapture",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        children.push(child);
    }

    for mut c in children {
        let status = c.wait().expect("wait child");
        assert!(status.success(), "child exited non-zero: {status}");
    }

    let source = RedbSource::new(&db).with_app_name(APP);
    let got = source.collect().expect("collect");
    let table = got.as_table().expect("table");

    let expected = usize::try_from(1 + WORKERS * OPS).unwrap();
    assert_eq!(
        table.len(),
        expected,
        "wrong row count after multi-process writes"
    );

    for w in 0..WORKERS {
        for j in 0..OPS {
            let key = format!("w{w}_k{j}");
            assert_eq!(
                table[&key],
                ConfigValue::Integer(w * 1000 + j),
                "wrong value for {key}"
            );
        }
    }
}
