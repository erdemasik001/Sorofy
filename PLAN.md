# MVP Yol Haritası — Soroban Contract Verification Service

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
6. Uçtan uca test: 2-3 farklı testnet contract API üzerinden doğrulanıyor

**Çıktı:** API local'de çalışıyor, curl ile doğrulama yapılabiliyor, sonuçlar cache'leniyor.

---

## Day3 — Deploy, Demo, Sunum

**Amaç:** Herkese açık, çalışan bir demo + jüri sunumu.

1. API'yi Docker-in-Docker destekleyen bir platforma deploy et (Fly.io / Railway / kendi VPS — build sandbox'ı Docker gerektirdiği için platform seçimi önemli)
2. Minimal kullanım örneği: statik sayfa yerine README + curl örnekleri yeterli (tam explorer UI MVP'de yok)
3. Multi-verifier ve retroactive verification için mimari/roadmap notu ekle (RFP'nin öne çıkardığı ama MVP'de çözülmeyen noktalar — README'de "Next" bölümü)
4. 2-3 gerçek testnet contract ile canlı demo kaydı (video/GIF)
5. Sunum materyalleri: mimari diyagram, "why us" (brief bölüm 5), sonraki milestone planı (brief bölüm 8: M2/M3)
6. Buffer / bug-fix zamanı

**Çıktı:** Canlı API + demo + submission-ready repo.

---

## İlerleme takibi

Bir günü bitirdiğinde "day0'ı bitirdik" gibi bir talimatla ilerleyeceğiz; her gün sonunda çıktıyı birlikte doğrulayıp bir sonraki güne geçeceğiz.
