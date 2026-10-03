pub mod config;
pub mod probe;
pub mod utils;

fn main() {
    println!("NetTrigger starting...");

    // 尝试安全加载配置
    match config::Config::load_or_create() {
        Ok((cfg, created)) => {
            if created {
                println!("已自动创建默认配置文件 config.toml");
            } else {
                println!("已成功加载配置文件");
            }

            // 初始化权威探针并快速进行一次连通性自检
            let probe = probe::Probe::new(&cfg.probe);
            let report = probe.check_with_report();
            println!(
                "探针自检结果: {:?} (延迟: {}ms)",
                report.status, report.latency_ms
            );
        }
        Err(err) => {
            eprintln!("加载配置文件失败: {}", err);
        }
    }
}
