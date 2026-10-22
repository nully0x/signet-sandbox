// Debug probe: run one exec against a pod and print the raw kube-rs error.
use k8s_openapi::api::core::v1::Pod;
use kube::{
    Client,
    api::{Api, AttachParams},
};

#[tokio::main]
async fn main() {
    let namespace = std::env::args().nth(1).expect("namespace");
    let pod = std::env::args().nth(2).expect("pod");
    let command: Vec<String> = std::env::args().skip(3).collect();

    let client = Client::try_default().await.expect("kube client");
    let pods: Api<Pod> = Api::namespaced(client, &namespace);
    let attached = pods
        .exec(
            &pod,
            command,
            &AttachParams {
                stderr: true,
                ..Default::default()
            },
        )
        .await;

    match attached {
        Ok(mut process) => {
            let mut stdout = process.stdout().expect("stdout");
            let mut buf = Vec::new();
            match tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf).await {
                Ok(_) => println!("OK: {}", String::from_utf8_lossy(&buf)),
                Err(e) => println!("READ-ERR: {e}"),
            }
        }
        Err(e) => println!("EXEC-ERR: {e:#}"),
    }
}
