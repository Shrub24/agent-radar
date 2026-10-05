#![cfg(unix)]

use agent_radar::{Collector, CollectorConfig, ObservationState};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

#[test]
fn shutdown_does_not_wait_for_descendants_holding_output_pipes() {
    let dir = std::env::temp_dir().join(format!("radar-descendants-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("herdr");
    let ready = dir.join("ready");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nsleep 2 &\nprintf ready > '{}'\nwait\n",
            ready.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let mut collector = Collector::new(CollectorConfig {
        executable,
        command_timeout: Duration::from_secs(5),
        ..CollectorConfig::default()
    });
    collector.tick(&mut ObservationState::new(), false);
    let deadline = Instant::now() + Duration::from_secs(1);
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        ready.exists(),
        "fake command must have launched its descendant"
    );
    let start = Instant::now();
    collector.shutdown();
    let elapsed = start.elapsed();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        elapsed < Duration::from_millis(300),
        "shutdown took {elapsed:?}"
    );
}
