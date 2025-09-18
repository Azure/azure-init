# azure-init behavior overview

This document describes the behavior of the reference `azure-init` binary with respect to user creation, password handling, and SSH configuration. Library consumers can customize behavior via `libazureinit` APIs and configuration.

## Summary

- By default, the reference `azure-init` binary does not set a password. It always creates the user and then locks the account with `passwd -l` because it never opts in to password provisioning.
- Library consumers may opt in to password provisioning by calling `User::with_password(...)`, which sets the password using `chpasswd` via stdin.
- `sshd_config` `PasswordAuthentication` is set according to IMDS `disablePasswordAuthentication` only when the real password backend is active (default backend is `passwd`).

## User and password behavior

- The password provisioner has two modes:
  - **Password provided (`User.password = Some`)**: Set the password securely with `chpasswd` by piping "username:password" to stdin. Nothing sensitive is placed on argv or logs.
  - **Password absent (`User.password = None`)**: Lock the account with `passwd -l`.

- The reference `azure-init` binary never calls `User::with_password`, so the constructed `User` has `password = None`. As a result, the password provisioner always takes the lock path (`passwd -l`). There is no alternate mechanism elsewhere that performs the lock.

- Build-time path to `passwd` is provided via the `PATH_PASSWD` environment variable (see `libazureinit/build.rs`).

### Relevant API usage in the binary

- `azure-init` builds a `User` without a password and passes it into provisioning alongside IMDS' `disablePasswordAuthentication` flag:

```356:448:src/main.rs
async fn provision(
    config: Config,
    vm_id: &str,
    opts: Cli,
) -> Result<(), anyhow::Error> {
    // ...
    let im = instance_metadata
        .clone()
        .ok_or::<LibError>(LibError::InstanceMetadataFailure)?;

    let user =
        User::new(username, im.compute.public_keys).with_groups(opts.groups);

    Provision::new(
        im.compute.os_profile.computer_name,
        user,
        config,
        im.compute.os_profile.disable_password_authentication,
    )
    .provision()?;
    // ...
}
```

### How passwords/locking are applied

- The password provisioner executes one of the following based on `User.password`:

```49:96:libazureinit/src/provision/password.rs
#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    if let Some(ref password) = user.password {
        // Set password via chpasswd (stdin)
        // ...
    } else {
        // No password provided; lock the account
        let path_passwd = env!("PATH_PASSWD");
        let mut command = Command::new(path_passwd);
        command.arg("-l").arg(&user.name);
        crate::run(command)?;
    }
    Ok(())
}
```

## SSH PasswordAuthentication

- When the real password backend is active (i.e., `password_provisioners.backends` includes `passwd`), `azure-init` writes an `sshd_config` drop-in that reflects IMDS `disablePasswordAuthentication`:
  - If IMDS `disablePasswordAuthentication` is `true`: write `PasswordAuthentication no`.
  - If `false`: write `PasswordAuthentication yes`.
- If a fake backend is configured (e.g., during tests), `sshd_config` is not modified.

```78:113:libazureinit/src/provision/mod.rs
self.config
    .password_provisioners
    .backends
    .iter()
    .find_map(|backend| match backend {
        PasswordProvisioner::Passwd => {
            PasswordProvisioner::Passwd.set(&self.user).ok()
        }
        #[cfg(test)]
        PasswordProvisioner::FakePasswd => Some(()),
    })
    .ok_or(Error::NoPasswordProvisioner)?;

let ssh_config_update_required = self
    .config
    .password_provisioners
    .backends
    .first()
    .is_some_and(|b| matches!(b, PasswordProvisioner::Passwd));

if ssh_config_update_required {
    let sshd_config_path = ssh::get_sshd_config_path();
    ssh::update_sshd_config(
        sshd_config_path,
        self.disable_password_authentication,
    )?;
}
```

## FAQ

- **Q: "only creates locked accounts." On what condition?**
  - **A:** When `User.password` is not specified. In the reference binary, `User::with_password` is never called, so `User.password` is always `None` and the account is consistently locked via `passwd -l`.

- **Q: Is it simply not specifying `user.password` and always incurring `-l`?**
  - **A:** Yes. Locking is performed by the same password provisioner that would set a password; it detects the absence of a password and runs `passwd -l`.

- **Q: Is the account locked by some other mechanism if the password path isn’t called?**
  - **A:** No. There is no alternate locking path; it is handled in the password provisioner itself.

## Security notes

- Passwords (when opted-in by library consumers) are set using `chpasswd` via stdin to avoid exposing secrets in process arguments or logs. 