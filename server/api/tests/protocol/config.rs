use clap::Parser;
use porchlight_api::config::ServeArgs;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    args: ServeArgs,
}
fn parse(extra: &[&str]) -> ServeArgs {
    Cli::try_parse_from(
        [
            "test",
            "--database-url",
            "postgres://localhost/test",
            "--enroll-host",
            "localhost",
            "--enroll-cidrs",
            "127.0.0.0/8",
        ]
        .into_iter()
        .chain(extra.iter().copied()),
    )
    .unwrap()
    .args
}
#[test]
fn request_pool_defaults_to_sixteen_and_is_configurable() {
    assert_eq!(parse(&[]).db_pool_size, 16);
    let args = parse(&["--db-pool-size", "2"]);
    assert_eq!(args.db_pool_size, 2);
    args.validate().unwrap();
    assert!(parse(&["--db-pool-size", "0"]).validate().is_err());
}
