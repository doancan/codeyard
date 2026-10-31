# Registry setup checklist

What the owner buys, creates and sends back to put the public extension registry (Yard) online, and the order of the
go-live steps. How it runs afterwards is in [registry.md](registry.md#running-the-public-registry). Everything else
(the server's setup, timers, backups, alerts, the canary, the rehearsal) is in the repository and tested.

Never paste a secret (a token, a password, a private key) into a chat, an issue or a commit. Where a step says
"on the server", type it there yourself, or share it with a Bitwarden Send that expires in a day.

## 1. Buy and create

| What | Choice | Cost |
|---|---|---|
| Server | Hetzner Cloud, a **CX22** (2 vCPU, 4 GB RAM, 40 GB disk) or the current shared-vCPU model of that size, **Ubuntu 24.04**, location **Nuremberg** or **Falkenstein**, with your SSH key, a public IPv4 and IPv6. Turn on Hetzner's *Backups* if you want a second copy (optional). Name it `yard`. | about 5 € a month (+1 € with Backups) |
| Domain | Any domain you own, with its DNS at Cloudflare. The registry is `yard.<domain>`. If you buy a new one, Cloudflare Registrar sells at cost. | about 12 $ a year |
| Cloudflare | A free account with the domain as a zone (nameservers changed at the registrar until the zone is *Active*). | free |
| Cloudflare R2 | Turn R2 on (it asks for a card; the free tier covers this). Create a bucket `codeyard-yard-backup`, location hint *Europe*. | free up to 10 GB |
| Alerts | One of: a Slack, Discord or Mattermost incoming webhook, an ntfy.sh topic URL, or an SMTP account that can send mail (host, port, user, password, from and to addresses). | free |
| Root keys | Two USB sticks (any size), and your Bitwarden account with two-step login on. | about 15 € once |

Total: about 6 € a month and the domain.

## 2. Send back

| Item | How | Used for |
|---|---|---|
| Server IPv4 and IPv6 addresses, and that `ssh root@<ipv4>` works from this Mac | in the chat | everything on the server |
| The domain | in the chat | `yard.<domain>` |
| A Cloudflare API token | Cloudflare → My Profile → API Tokens → Create Token → *Custom*. Permissions: **Zone · DNS · Edit**, **Zone · SSL and Certificates · Edit**, **Zone · Zone Settings · Edit**, all for this one zone. TTL a week. Bitwarden Send. | the DNS record, the origin certificate, SSL mode *Full (strict)*, *Always Use HTTPS*, minimum TLS 1.2 |
| The R2 account ID, and an R2 API token for the bucket | R2 → Manage API tokens → Create → *Object Read & Write*, applied to `codeyard-yard-backup` only. Its *Access Key ID* and *Secret Access Key*: type them into `/etc/yard/yard.env` on the server at step 4, or Bitwarden Send. | backups |
| The alert channel | the webhook URL, or the SMTP URL (`smtps://host:465` or `smtp://host:587`), user, password, from and to: on the server at step 4, or Bitwarden Send | alerts |
| A copy of `/etc/yard/yard.env` | after step 4, as a Bitwarden secure note "Codeyard Yard server env" | restoring on a new server |

In the Cloudflare dashboard, check yourself: **Security → Bots → Bot Fight Mode off** (it would challenge `curl` and
`cyx`, which are the registry's clients).

Who holds what after the ceremony (also in [SECURITY.md](../SECURITY.md#extension-registry-keys)):

| Key | Held by | Kept |
|---|---|---|
| Root A | the owner | encrypted USB stick A, at home |
| Root B | the owner, until a second maintainer takes it | encrypted USB stick B, somewhere else |
| Root C | the owner | Bitwarden secure note "Codeyard Yard root key C" |
| Online key | the server | `/srv/yard/keys/online.key`; never backed up, replaced by a rotation |
| Canary and deploy SSH keys | GitHub environment `yard` | secrets; each can run one command on the server |

## 3. Go live

The owner does steps 5 and 7 (the keys never leave the owner's hands); the commands in the other steps are run from
this Mac with the owner's SSH key and the token above.

1. **Rehearse** (any time): `deploy/rehearse.sh` passes on this Mac and in CI.
2. **Build**: run the `yard-deploy` workflow with *install* off; download the `yard-linux-x86_64` artifact.
   ```text
   gh workflow run yard-deploy -f install=false        (wait for it, then:)
   gh run download "$(gh run list -w yard-deploy -L 1 --json databaseId -q '.[0].databaseId')" -n yard-linux-x86_64 -D deploy/yard/
   ```
3. **Cloudflare and the server**: an origin certificate for `yard.<domain>` (Cloudflare API, 15 years) saved as
   `deploy/yard/origin.pem` and `origin.key` (ignored by git, like the `yard` binary there); DNS `A` and `AAAA` records `yard` → the server,
   **proxied**; SSL mode *Full (strict)*, *Always Use HTTPS* on, minimum TLS 1.2. Then:
   ```text
   scp -r deploy/yard root@<ip>:/root/yard-setup
   ssh root@<ip> /root/yard-setup/setup.sh yard.<domain>
   ssh root@<ip> shred -u /root/yard-setup/origin.key
   ```
4. **Secrets**: the owner fills in `/etc/yard/yard.env` on the server (`sudoedit /etc/yard/yard.env`), then:
   ```text
   ssh root@<ip> 'echo "This is a test alert from the registry." | yard-alert "a test"'      (the alert arrives)
   ssh root@<ip> systemctl restart litestream
   ```
5. **Key ceremony** (the owner, at this Mac, network off): see [registry.md](registry.md#root-key-ceremony).
   ```text
   cargo build --release -p cyx
   CYX=target/release/cyx scripts/yard-root-ceremony.sh /Volumes/YARD-A /Volumes/YARD-B
   ```
6. **The registry**, with the three public keys from `yard-ceremony-record.txt`:
   ```text
   ssh root@<ip> sudo -u yard yard init /srv/yard --registry yard.<domain> --root-keys A,B,C --threshold 2
   scp root@<ip>:/srv/yard/root.json .
   ```
7. **Sign the root** (the owner, two keys):
   ```text
   cyx yard sign root root.json --key /Volumes/YARD-A/yard-root-a.key
   pbpaste | cyx yard sign root root.json --key -        (key C, copied from Bitwarden)
   ```
8. **Start**:
   ```text
   scp root.json.sig root@<ip>:/srv/yard/ && ssh root@<ip> chown yard:yard /srv/yard/root.json.sig
   ssh root@<ip> systemctl start yard litestream yard-resign.timer yard-watch.timer yard-backup.timer yard-restore-check.timer
   cyx yard check https://yard.<domain> --root-key A --root-key B --root-key C --threshold 2
   ssh root@<ip> 'yard-watch && yard-backup && yard-restore --check'
   ```
9. **GitHub**: environment `yard` (required reviewer: the owner) with variables `YARD_URL`, `YARD_ROOT_KEYS`
   (`A,B,C`), `YARD_THRESHOLD` (`2`), `YARD_SSH_HOST`, `YARD_SSH_KNOWN_HOSTS` (the same three URL and key variables
   at repository level for `yard-monitor`), and two new SSH keys as secrets `YARD_CANARY_SSH_KEY` and
   `YARD_DEPLOY_SSH_KEY`, their public halves authorized on the server
   ([registry.md](registry.md#runbook), "Rotate the canary or deploy key").
10. **Canary**: run `yard-canary` by hand once (it also claims the `codeyard` publisher name), then let it run weekly.
    Exit criterion: three passing runs in a row in `canary.tsv`.
11. **The app**: a pull request that sets `PUBLIC` in `crates/app/src/ext_registry.rs` and `DEFAULT_REGISTRY` in
    `crates/cyx/src/publish.rs` to `yard.<domain>`, adds the public keys and the ceremony date to SECURITY.md, and
    attaches the signed ceremony record. The owner signs off the record; the next release ships the keys.
12. **Expired index on a live server** (optional; the rehearsal already checks it): on the server
    `sudo -u yard yard init /tmp/staging`, `"listen": "127.0.0.1:8081"` in its `yard.json`, `yard serve` and
    `yard resign --expired` on it, then from
    this Mac through `ssh -L 8081:127.0.0.1:8081` with a client whose `extensions.registries` names
    `http://127.0.0.1:8081`; delete `/tmp/staging` after.

Data-only publishing opens after step 11's release. Code stays by invitation (`invited` in `yard.json`), with the 48 h
wait on clients and a human review of each publisher's first native program.
