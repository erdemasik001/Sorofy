# MVP Yol Haritası — Sorofy (Soroban Contract Verification)

> Kaynak: [idea1-project-brief.md](idea1-project-brief.md)
> Format: Solo geliştirici, 3 günlük hackathon + Day0 hazırlık günü.
> Hedef: Hackathon sonunda çalışan, canlıda erişilebilir bir MVP + sunuma hazır repo.

## MVP kapsamı (ne YAPILACAK)

- Tek bir doğrulama akışı: kaynak kod (git repo/commit) + hedef WASM hash → deterministic Docker rebuild → sha256 karşılaştırma
- Testnet contract'ları için çalışan bir REST API (`POST /verify`, `GET /verify/{id}`)
- Basit bir sonuç cache'i (SQLite/sled)
- Soroban testnet RPC'den on-chain WASM hash çekme
- Public olarak deploy edilmiş, canlı çalışan bir demo

## MVP kapsamı DIŞI (bilerek ertelenen, brief'teki M2/M3)

- Multi-verifier / decentralization mimarisi (sadece roadmap notu olarak belirtilecek)
- Retroactive on-chain registry (pre-SEP-58 contract'lar için)
- Tam bir explorer/wallet entegrasyonu veya UI
- Mainnet + audit süreci

Bu ayrım jüriye net anlatılacak: "MVP çekirdek problemi (source→bytecode proof) çözüyor; farklılaşma noktaları (decentralization, retroactive verification) planlanmış ve mimaride yer açılmış ama bu fazda scope dışı."

---

## Day0 — Hazırlık (hackathon öncesi)

**Amaç:** Ortamı kur, riskleri manuel olarak keşfet, repo iskeletini ve mimariyi dondur.

1. Ortam kurulumu: Rust + `wasm32v1-none` target, `stellar-cli`, Docker Desktop
2. SEP-58 spec'ini oku, alanları not al: `bldimg`, `bldopt`, `source_repo`, `source_rev`, `tarball_url`, `tarball_sha256`
3. Basit bir testnet contract seç (ör. `soroban-examples/hello-world` veya `increment`) — kaynağını ve deploy edilmiş WASM hash'ini not al
4. Bu contract'ı **manuel olarak** reproduce et (`stellar contract build` ile) ve sha256'ları karşılaştır. Nerede sürtünme çıkıyor (non-determinism, network fetch, toolchain versiyonu) gözlemle ve not al — Day1'in en büyük riski burada ortaya çıkar
5. GitHub repo iskeletini oluştur (README, LICENSE, .gitignore, Cargo workspace boş crate'ler)
6. Mimari diyagramı (mermaid) README'ye ekle
7. Bu dosyadaki scope'u gözden geçir, gerekirse daralt/dondur

**Çıktı:** Ortam çalışıyor, 1 contract manuel doğrulanmış, repo hazır, mimari net.

---

## Day1 — Deterministic Build Engine

**Amaç:** Kaynak + build parametrelerinden WASM üretip hash karşılaştıran çekirdek motor.

1. Digest-pinned Docker image: sabit Rust toolchain + `stellar-cli` versiyonu, `RUSTUP_TOOLCHAIN` pinlenmiş
2. Container izolasyonu: build sırasında network erişimi kapalı (dependencies önceden vendor/fetch edilmiş şekilde) — submitted code sandbox dışına çıkamamalı
3. `verifier-core` crate: `{source_repo, source_rev | tarball_url, package, bldopt}` al → clone/indir → container içinde build → çıkan `.wasm` dosyasının sha256'sı
4. Day0'da manuel doğrulanan contract ile otomatik testi doğrula — sonuç manuel sonuçla birebir eşleşmeli
5. Temel hata tipleri: build fail, timeout, hash mismatch

**Çıktı:** `verify-core --repo <url> --rev <sha> --wasm-hash <hash>` çalışıp `verified`/`mismatch` dönüyor.

---

## Day2 — Public API + On-chain Lookup + Cache

**Amaç:** Servisi REST API olarak dışarı aç, on-chain veriyi çek, sonucu sakla.

1. Axum ile REST API:
   - `POST /verify` — SEP-58 alanlarını + `contract_id` kabul eder, job başlatır
   - `GET /verify/{contract_id|wasm_hash}` — cache'lenmiş sonucu döner (`verified` / `mismatch` / `pending` / `not_found`)
2. Soroban testnet RPC client: `contract_id` → on-chain WASM hash
3. Basit persistence (SQLite/sled): `{contract_id, wasm_hash, source_info, result, timestamp}`
4. Job execution: MVP için senkron ya da basit in-memory `tokio` task kuyruğu yeterli
5. Response şemasına "trust level" alanı ekle (`arbitrary` / `publicly-auditable` / `sdf-maintained`) — MVP'de sabit değer dönebilir, ama şema hazır olsun (bu, brief'teki farklılaşma noktalarından biri)
6. **Gerçek-boyut determinism kontrolü (risk öne çekme):** anlamlı büyüklükte bir contract'ı (ör. `soroban-examples` token / atomic-swap, ~40 KB) kendi hesabından testnet'e deploy et ve motoru **on-chain hash'e karşı** doğrula. Day1 sadece 660 B'lik `hello-world`'ü reproduce etti; asıl mühendislik riski (build script, absolute path, codegen units) ancak gerçek boyutta ortaya çıkar. Kaynak da toolchain da senin elinde olduğu için bu saf bir determinism testi — `bldimg`/kaynak-tedarik riski yok. RPC client'ı zaten bu adımda yazılıyor, deploy aynı oturumda yapılır. Bozulursa tamir için Day3'e kadar süre var (Day3'te değil).
   - Deploy sonrası `stellar contract info meta` ile bu contract'ın `contractmetav0`'ını oku ve **SEP-58 alanlarının (`source_uri`/`bldimg`) gerçekten yok olduğunu doğrula** — Day0'ın `CDZZZTN6…` bulgusu (mevcut tooling bu alanları otomatik gömmüyor) burada da geçerli olmalı. Doğrularsa bu contract **Day3'ün retroactive path hedefi** olarak işaretlenir: kaynağı zaten elinde, arama/tedarik gerekmez.
7. Uçtan uca test: 2-3 farklı testnet contract API üzerinden doğrulanıyor

**Çıktı:** API local'de çalışıyor, curl ile doğrulama yapılabiliyor, sonuçlar cache'leniyor; motor gerçek-boyut (~40 KB) bir testnet contract'ını on-chain hash'e karşı byte-for-byte reproduce ediyor; bu contract, metadata'sında SEP-58 alanı taşımadığı doğrulanarak Day3'ün retroactive hedefi olarak kilitleniyor.

---

## Day3 — Deploy, Demo, Sunum

**Amaç:** Herkese açık, çalışan bir demo + jüri sunumu.

1. API'yi Docker-in-Docker destekleyen bir platforma deploy et (Fly.io / Railway / kendi VPS — build sandbox'ı Docker gerektirdiği için platform seçimi önemli)
2. Minimal kullanım örneği: statik sayfa yerine README + curl örnekleri yeterli (tam explorer UI MVP'de yok)
3. **Retroactive path — Day2'nin hedefini yeniden kullan.** Day2, item 6'da deploy edilen ve metadata'sında SEP-58 alanı olmadığı doğrulanan contract'ı retroactive girdi olarak besle: kaynağı + `bldimg`'i (zaten Day2'de kullanılan image) dışarıdan tedarik edilmiş gibi ver, API'nin onu `verified` döndürdüğünü doğrula. Kaynak zaten elinde olduğu için ayrı bir "uygun kaynak bul" arama adımına gerek yok — tedarik riski Day2'de kapatıldı. Day0'daki `CDZZZTN6…` (gerçek ekosistem contract'ı, kaynağı hâlâ bilinmiyor) bonus/stretch hedef olarak kalır — çekirdek iddia ona bağlı değil.
4. 2-3 gerçek testnet contract ile canlı demo kaydı (video/GIF)
5. Sunum materyalleri: mimari diyagram, "why us" (brief bölüm 5), sonraki milestone planı (brief bölüm 8: M2/M3)
6. Buffer / bug-fix zamanı

**Çıktı:** Canlı API + demo + submission-ready repo.

---

## İlerleme takibi

Bir günü bitirdiğinde "day0'ı bitirdik" gibi bir talimatla ilerleyeceğiz; her gün sonunda çıktıyı birlikte doğrulayıp bir sonraki güne geçeceğiz.
