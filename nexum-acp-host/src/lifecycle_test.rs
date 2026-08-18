use super::lifecycle;

#[test]
fn test_host_and_local_client_share_runtime_directory() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = temp.path().join("nexum");
    let socket = lifecycle::socket_path_in_runtime(&runtime);
    let local_socket = runtime.join("acp.sock");

    assert_eq!(socket, local_socket);
    // La base de cron YA NO vive junto al socket: es dato persistente del
    // usuario y el runtime dir es volátil. Se verifica lo contrario de antes.
    assert_ne!(
        lifecycle::cron_store_path().parent(),
        socket.parent(),
        "cron.db no puede vivir en el runtime dir: se borra al cerrar sesión"
    );
    assert_eq!(
        lifecycle::legacy_cron_store_path_in_runtime(&runtime).parent(),
        socket.parent()
    );
}

#[tokio::test]
async fn stale_socket_is_removed_only_after_owner_exit() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("acp.sock");
    let listener = lifecycle::bind(&socket).await.unwrap();
    let inode = lifecycle::socket_inode(&socket).unwrap();

    assert!(!lifecycle::remove_owned_socket(&socket, inode + 1).unwrap());
    assert!(socket.exists(), "a non-owner inode must not be removed");
    drop(listener);
    assert!(lifecycle::remove_owned_socket(&socket, inode).unwrap());
    assert!(!socket.exists());
}
