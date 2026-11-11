# 匯入紀錄

Reach 的初始程式與文件來自一個**不是 git** 的既有目錄（沒有 commit 歷史）。本檔記錄來源、複製方式、排除項目，以及每個來源檔的 sha256。

## 來源

- 來源：作者 Mac 上另一個本機、非公開專案內的通用層目錄（不是 git 倉庫）。本庫不記錄它的路徑或名稱。
- 複製時間：2026-11-09 16:38:11 CST（Asia/Taipei），即 2026-11-09 08:38:11 UTC。
- 來源最新 mtime：2026-11-09 16:06:54 CST（`BACKGROUND-INPUT.md`），複製時已超過 30 分鐘沒有變動；複製前後各列一次來源檔案修改時間，兩次一致（複製期間來源未變動）。
- 複製命令：`rsync -a --exclude target/ --exclude .DS_Store <來源>/ <暫存目錄>/`，再依下表放入本庫。`-a` 保留檔案修改時間。
- 來源未被修改、移動或刪除；工作結束時重新核對（見 PR 說明）。

## 排除項目

- `target/`（建置產物，約 712 MB）、`.DS_Store`。
- 來源所在專案的其餘內容（截圖、評測、資料集、帳號、素材與規則文件等）一律未匯入，也不屬於 Reach。

## 原始歷史證據

文件原本連結到來源專案的 `benchmark/` 與 `screenshots/`。這些證據**未隨本庫攜帶**，文件中的連結已改為純文字（「原始證據：來源專案 …」）。要重現當時的紀錄，需要來源專案或另行整理的證據。

## 檔案放置

- 根目錄的 Rust 與 Swift 檔維持原位（`build-*.sh` 以 `cd "$(dirname "$0")"` 與根目錄相對的檔名建置）。
- 來源的 `*.md` 移到 `docs/`；來源 `README.md` → `docs/CAPABILITIES.md`，`STANDALONE.md` → `docs/BUILD.md`。本庫根目錄的 `README.md` 為新寫。

## 狀態說明與清單

共 84 個來源檔，其中 35 個在匯入後被修改。狀態 `copied` 代表本庫檔案與來源 sha256 相同；`modified` 代表已修改，表中 sha256 仍是**來源**的值。本庫在 2026-11-10 以單一 commit 重建，匯入當時的逐步 commit 不在本庫。修改的原因依序是：文件獨立化與 agent 中立、遊戲字樣中性化、`cargo fmt` 與 clippy 的機械修改。

| sha256（來源檔） | 來源路徑 | 本庫路徑 | 狀態 |
|---|---|---|---|
| f6a3dfe389a15c862d4ac2f8df3abffbce98370216d49c980e478f7f4af59fcd | `AXPressExperiment.swift` | `AXPressExperiment.swift` | copied |
| 1e6ef43472e5b33a767b670015a535b94d6229aa7722bc3f0bb78cfd99cca206 | `AccessibilitySnapshot.swift` | `AccessibilitySnapshot.swift` | copied |
| 6c3d98e2b0b1e543271bebeaa79f0d399a2131295e71737f43a6bdbb51859c66 | `FocusNotifications.swift` | `FocusNotifications.swift` | copied |
| e9a3d1794b728d15107f2c08536c177e89faf82c035b8c32ac4c859c28d43204 | `FrameStatusProbe.swift` | `FrameStatusProbe.swift` | copied |
| b486a5764b2efef827721e6e79d013766a352a6735299c04706667023aae8daa | `HeadlessEventExperiment.swift` | `HeadlessEventExperiment.swift` | copied |
| ba9b0315497ef622145baac541da569a987601e775e60cdf9764c2a7641b3653 | `HiddenAppKitExperiment.swift` | `HiddenAppKitExperiment.swift` | copied |
| 909ae0ce33edd351244da03e3bf974262ec5ae38777aa4be2543d2e01e33d5e9 | `HiddenWindowEventExperiment.swift` | `HiddenWindowEventExperiment.swift` | copied |
| 1f2c74a1cbb51f580e620816485f4fec620bbb053472f60fdc24f936060cb67e | `ObserveIsolation.swift` | `ObserveIsolation.swift` | copied |
| 772fa233ad14a6304b0ca542f43f56edcff83d99cd9da771dfc05a0b46faa54a | `RecognizeText.swift` | `RecognizeText.swift` | copied |
| 49fb2b11880b7db659f896f7467fd27f22168e9d85e1b8a24661de792707be16 | `SessionStatus.swift` | `SessionStatus.swift` | copied |
| 359a2bdf847cbb644e4e7e636d173ac56f0036c1c0894916320fc6b56d97da54 | `StreamImageCapture.swift` | `StreamImageCapture.swift` | copied |
| 93e8f95e06ed827b3c3aeedb8d789168c59a661aceb2c20f716ce9021c84a29a | `TargetEventExperiment.swift` | `TargetEventExperiment.swift` | copied |
| 7172ec4fbfdab38386d0ccfebc1d2d25a283a0d2f990ad00c636e141c5c268bf | `WindowCapture.swift` | `WindowCapture.swift` | copied |
| 720da0563e23b2c11c7f76d9b95a1c00ab295ff29c5890ef0301dad0eb99658e | `build-ax-press-fixture.sh` | `build-ax-press-fixture.sh` | copied |
| e208962bccce7daf193f65e271c82354336e95763fe087c29c376f15e5036582 | `build-ax.sh` | `build-ax.sh` | copied |
| e0e1ba2b6799ac0de0b15de11fb53d993ff9b2d659e51160f2b94dde5406a713 | `build-capture.sh` | `build-capture.sh` | copied |
| f054737c1dc0c7897dc4eb14ac8f0374c2371f1009c708876a8f6eae3a40247f | `build-focus-notifications.sh` | `build-focus-notifications.sh` | copied |
| 7dec04e07c86378575442a4a54b9d0c87ccd3a22d21b4e5d970a60bfa68593c1 | `build-frame-status.sh` | `build-frame-status.sh` | copied |
| 5e9efac91e65ae823948d2af64f62a9f7a98e8b5d20243a31969207dea0443ba | `build-headless-event-fixture.sh` | `build-headless-event-fixture.sh` | copied |
| 86f0d1c95fd05c5f94836ad1bdf67bffa5d1be7e8b5179aea7888d30fc82876f | `build-hidden-appkit-fixture.sh` | `build-hidden-appkit-fixture.sh` | copied |
| 848c87ed4259101d5907c94cd893261e58e2c91f75d6e8932781e47fa51cd1ed | `build-hidden-window-fixture.sh` | `build-hidden-window-fixture.sh` | copied |
| 9266e2cb938ab8b9e4df543004474b9cfd7710ed2add0c8ddd2b5b2a71286a18 | `build-ocr.sh` | `build-ocr.sh` | copied |
| b6c6c5ffc6edeec64dcebb7f8d44228ad5679f608131e8a74fc61e41168eadda | `build-session-status.sh` | `build-session-status.sh` | copied |
| 37b26c4d0ce0fb87eb98db67acee1bceb0153f375fdc072e2fd6b22e98338fe0 | `build-stream-image.sh` | `build-stream-image.sh` | copied |
| ee44041d13ee086255be3986420641ec788720b2868255aea698d2915d9d1916 | `build-target-event-fixture.sh` | `build-target-event-fixture.sh` | copied |
| cea2fc0d9b69ea66d5304e13ec391e5501a77963d5a16e3289d4f2d0dc25e0f0 | `test-headless-event-fixture.sh` | `test-headless-event-fixture.sh` | copied |
| d96e9b63e12ac456f59edd22aa8d596514651a15d28861aa923f7dc8d32bfa95 | `test-focus-notifications-live.py` | `test-focus-notifications-live.py` | copied |
| 47c8cd407eef6b5e46b817ae2cd39da70e70cfc70ade6a64d0e849b0036939fe | `test-legacy-guards.py` | `test-legacy-guards.py` | copied |
| f0da9eab38e2eb349eae356be0e54bdd1fd45be0ac996c4727265e89e721b8df | `AGENT-CLI.md` | `docs/AGENT-CLI.md` | modified |
| f336ab5e846b91f3bafddfd89daed04c7f07a514ea5e2390563b238425941aab | `BACKGROUND-INPUT.md` | `docs/BACKGROUND-INPUT.md` | modified |
| 63acdbd210bcf77a5b30df9172fdf34c4d0656f17d4fa3eaed90f125256484e5 | `ISOLATED-SESSION.md` | `docs/ISOLATED-SESSION.md` | modified |
| c8b511d7d375ef6f8563731a2a0234df00f49acdd2428550140928a3db00e532 | `ISOLATION-OBSERVATION.md` | `docs/ISOLATION-OBSERVATION.md` | modified |
| feb1f5624668d4a3b451ed6ab5d62c3775839801ad10c9c2a11a4d1f1b13014c | `JOINT-OBSERVATION.md` | `docs/JOINT-OBSERVATION.md` | modified |
| 9e1cd31cde00f92679544d41ad21ffc21df32f430c1711b2fbfecaab568ea8c3 | `OCR.md` | `docs/OCR.md` | modified |
| 4e69e1cc08f4580a2070c020046c6e34def9b49ee7388f03e0bcd2f40fe7ad56 | `README.md` | `docs/CAPABILITIES.md` | modified |
| 149c4ff51b145ed117c4e421a432f790aca809ec8a66026628c17d64d070cf48 | `STANDALONE.md` | `docs/BUILD.md` | modified |
| 452b456d287f063c3ad186a7bf178a20829af8b154fd86b08b41609b099f258f | `VIRTUAL-INPUT.md` | `docs/VIRTUAL-INPUT.md` | copied |
| 233376ab2e1add2019c31390cc8167653f8d38e7acdf1090d64ed592bc537c33 | `Cargo.toml` | `Cargo.toml` | copied |
| 9822306f4ead748d6f30719a2bb679a3297f56a5baa79f1e69afcff915c82dc4 | `Cargo.lock` | `Cargo.lock` | copied |
| 384b0d69cbe34b9273b374a68671116d05c9674d32741461debae49cb809c6c7 | `src/ax_cg.rs` | `src/ax_cg.rs` | modified |
| 73cb978f3efd0b1601550d882613f30736d917096fa4db474a50490446dc20e2 | `src/bounded_process.rs` | `src/bounded_process.rs` | modified |
| 2051291bf36d2416ffd1e95cfc688a00c3c15bf3a93bc1cc00d8135867d39b46 | `src/capture_geometry.rs` | `src/capture_geometry.rs` | modified |
| 266fa9a92e83f2f450daf629c384cb91901928a69c4d8c954f71897462a6ca51 | `src/capture_lock.rs` | `src/platform/macos/capture_lock.rs` | modified |
| e3e5f513448535775f306f56a02c4c61ea064f0093cf491a4400568d476d67dc | `src/display_inventory.rs` | `src/display_inventory.rs` | modified |
| effdbb9e457138e4d5bfecc9fceddbabd712bd1e607cd7a980076c6e05106b1f | `src/input_guard.rs` | `src/input_guard.rs` | modified |
| d855a1c3ad79ad3054cc27e3eb2e789adc25e93029fa36cc0806a7876296c730 | `src/joint_observation.rs` | `src/joint_observation.rs` | modified |
| 43ff3e7c17bf5459e3e13983181905ae86d6abad53d4a0810c2f59cb74801039 | `src/lib.rs` | `src/lib.rs` | modified |
| 7bc8d8205c674b0ac44a6afe281aab1f3171200a9cb71aa03b60e3b7c1317cbb | `src/live_ax_cg.rs` | `src/live_ax_cg.rs` | modified |
| 40296aac6163ff217fdcc5337c35a0fa8949939aa29f19e56e78d1d8f64121b1 | `src/macos.rs` | `src/platform/macos/windows.rs` | modified |
| ce4d16e06829dc75874ac698e9e1e80e347e75096eceaf325735edfabbbe44e1 | `src/main.rs` | `src/main.rs` | modified |
| e6325b032c065a8d2f1e104d6e57b664aea6ed55bf72fda33b132b2e6d34f68f | `src/native_event_preview.rs` | `src/native_event_preview.rs` | modified |
| cb26427812164810faacc14771fe22084ff8a741823bf14e8a62ee28c2d1abfb | `src/simulate.rs` | `src/simulate.rs` | modified |
| 7c805e6cefe3cb72a7bdd7c7e8ef00c7504d51f093043b48c779b1879a18be08 | `src/stream_image.rs` | `src/platform/macos/stream_image.rs` | modified |
| 32843d9ab78049dc99fd9dae151a68557e033fc1c72e9f6d292bb68adaa6be5d | `src/stream_status.rs` | `src/platform/macos/stream_status.rs` | modified |
| b9302873204343ee6182c71ff1371a9add937edffcf182944c15af60eec832af | `src/strict_json.rs` | `src/strict_json.rs` | copied |
| 17bdf201d9aa7f81ee5fee63036872bdd4b9a1541390d4b9f9d2404f043a0e6d | `src/virtual_input.rs` | `src/virtual_input.rs` | copied |
| d8182914dc313dcc97393e86cae8336e75ac66d0a4e29401abf9d1abe645b446 | `src/bin/cu-agent.rs` | `src/bin/cu-agent/app.rs` | modified |
| c396a3204e48789ec2b35c35c42d2910ee43eb3ef1b5e1b88ca045d168006cd1 | `src/bin/cu-focus-watch.rs` | `src/bin/cu-focus-watch.rs` | modified |
| 57053fae057ae59aed2654644e0de8c369baf302f50da0ce4e5feb5184e864b7 | `src/bin/cu-headless-check.rs` | `src/bin/cu-headless-check.rs` | modified |
| 39cce12c95706b474edc70265849747b8fa8a2b0832c8fd1b958f7e32e22f025 | `src/bin/cu-hidden-appkit-check.rs` | `src/bin/cu-hidden-appkit-check.rs` | modified |
| b3d10ee5f7c6ed6703fa2937edf5aaedb7bc00036d7ac715b8681e2bcec522c4 | `src/bin/cu-hidden-window-check.rs` | `src/bin/cu-hidden-window-check.rs` | modified |
| 257480f5de2fc23e54725999154d8dd6de1eaff11e7a6892beda32430ff9416c | `src/bin/display-inventory.rs` | `src/bin/display-inventory.rs` | copied |
| af6bd778b14648e3d8b4cc76df3ac2b114faf97e5e6c2c055796c39b68702810 | `tests/cli.rs` | `tests/cli.rs` | modified |
| 9c1c47f46fb43c0a35d1d8806abec7a39634a62b04712665c54b3117ec50e40a | `tests/cu_agent_cli.rs` | `tests/cu_agent_cli.rs` | modified |
| 0987559ee15b54eb8745a419ab7cf42aea2f780db8ac00b91fbc67d70c4a702b | `tests/focus_watch_cli.rs` | `tests/focus_watch_cli.rs` | modified |
| 14e5b06cc0a5a016e599186de8f1492b59c2b35a1a5104f040043781b51d0a81 | `tests/headless_check_cli.rs` | `tests/headless_check_cli.rs` | modified |
| 1b22e682a93c2319661825e9500a1fbbb12c7d01ad374a3318d574051a8d92e6 | `tests/hidden_appkit_check_cli.rs` | `tests/hidden_appkit_check_cli.rs` | modified |
| 87aa64a782af0ef1fbda93f78c0de7604dd2d7bd07b52aa758ad79407993da4a | `tests/hidden_window_check_cli.rs` | `tests/hidden_window_check_cli.rs` | modified |
| 11abed34f0eacd0000680d93d293ce1ff21392359a3ba21a8a89b12e97ad624e | `tests/macos_event_memory.rs` | `tests/macos_event_memory.rs` | modified |
| 56339fa4f8598db152fac4d944d70fe58cf5e8982bc8e82405aed3845d11ed4b | `tests/virtual_input.rs` | `tests/virtual_input.rs` | modified |
| 78dc0338d80da8ead23e424da86aa84d797f2a35b3df45ac13ef05db1a4a4c0f | `fixtures/AXFixture.swift` | `fixtures/AXFixture.swift` | copied |
| e114e420adbaee4ba8e338027858863ad3e82fd01679e3b3d857956089194308 | `fixtures/AXPressFixture.swift` | `fixtures/AXPressFixture.swift` | copied |
| 9d84cc60cf071e4cece87d2db94fb14261f122d3951e54ee894ae00370cb4ae9 | `fixtures/AXPressInfo.plist` | `fixtures/AXPressInfo.plist` | copied |
| 2cda415e4b4e9e515e247582cd5fc15df3f151b3a699eb8de6346f9e313e2d69 | `fixtures/Info.plist` | `fixtures/Info.plist` | copied |
| 37b79cb29a70c9228b22213a3df3127d807ccace5ff5b4fd5ef062338e83e391 | `fixtures/NSApplicationFocusProbe.swift` | `fixtures/NSApplicationFocusProbe.swift` | copied |
| 55cedac6f027fb193f74b9fb6be8570d4631b2fef5283455079a72788a55c598 | `fixtures/PassiveActivationPolicyDiagnostic.swift` | `fixtures/PassiveActivationPolicyDiagnostic.swift` | copied |
| a71e14303a9e85980fd6ec14692781b73bc5ffc8221401be2c19f8a7a6d1f07b | `fixtures/PassiveFocusWarmupDiagnostic.swift` | `fixtures/PassiveFocusWarmupDiagnostic.swift` | copied |
| 18915d236b75d59d6216c8bbd8202e2da74753e164c236f941ec94cc625ab7da | `fixtures/PassiveNSAppDiagnostic.swift` | `fixtures/PassiveNSAppDiagnostic.swift` | copied |
| e989583b181c56f2640206563960c2f3f0a814eb0718f8288a4aa593a25c9670 | `fixtures/SessionStatusDiagnostic.swift` | `fixtures/SessionStatusDiagnostic.swift` | copied |
| 00a19788049c9e9bfa149915984708e89fca2123156438682483928d717d65ad | `fixtures/SystemFocusAttributeDiagnostic.swift` | `fixtures/SystemFocusAttributeDiagnostic.swift` | copied |
| a4df54d13ac142b0344edfe2ad0fa363c4d05d8c78e45ab41445addd6964bc26 | `fixtures/SystemFocusDiagnostic.swift` | `fixtures/SystemFocusDiagnostic.swift` | copied |
| 59f54d56e05b92f60d2dad78f068b936438223c074f2b6a53d806120ea831c0a | `fixtures/SystemFocusTimeoutDiagnostic.swift` | `fixtures/SystemFocusTimeoutDiagnostic.swift` | copied |
| cd4130112c7013a16325dd8d9592f6387e2e0494da05ebb257de84ab90cb76b2 | `fixtures/TargetEventFixture.swift` | `fixtures/TargetEventFixture.swift` | copied |
| d8028acaf022f559e54b89a43188fe977f35971eacce0ca3677dfc0318c729ae | `fixtures/TargetEventInfo.plist` | `fixtures/TargetEventInfo.plist` | copied |
