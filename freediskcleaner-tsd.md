# Tech Stack Document — FreeDiskCleaner

> Wygenerowano z PRD: `prd-freediskcleaner.md`
> Data: 2026-10-20

---

## 1. Przegląd

FreeDiskCleaner to darmowa, otwartoźródłowa (MIT) aplikacja okienkowa na macOS z Apple Silicon, która skanuje dysk, rozpoznaje developerskie śmieci i bezpiecznie je usuwa. Stack jest zoptymalizowany pod trzy rzeczy:

1. **Kod pisze agent AI (Claude Code)**, więc priorytetem jest to, co agent potrafi sam uruchomić, kliknąć i zweryfikować. Wybór technologii podporządkowano testowalności.
2. **Bezpieczeństwo operacji destrukcyjnych.** Narzędzie kasuje pliki użytkownika, więc logika ocen i usuwania jest odseparowana od UI, deterministyczna i testowana na sztucznych drzewach katalogów.
3. **Jeden język, jeden binarny artefakt.** Całość w Rust, UI w Slint, bez webview i bez drugiego runtime'u.

---

## 2. Architektura platformy

Aplikacja lokalna, bez backendu, bez kont, bez telemetrii i bez sieci (poza opcjonalnym sprawdzaniem aktualizacji, które w v1 jest poza zakresem).

```
┌─────────────────────────────────────────────┐
│  UI (Slint, pliki .slint)                    │  widoki: skan, tabela, mapa dysku,
│                                              │  potwierdzenia, ustawienia, O programie
├─────────────────────────────────────────────┤
│  crates/app  — glue: stan, kanały, wątki     │
├──────────────────────┬──────────────────────┤
│  crates/core         │  crates/cleaner       │
│  skaner, model,      │  Kosz, usuwanie,      │
│  silnik reguł, ocena │  Docker CLI, tmutil,  │
│  bezpieczeństwa,     │  simctl (adaptery)    │
│  treemap (układ)     │                       │
├──────────────────────┴──────────────────────┤
│  macOS: system plików APFS, Kosz, Full Disk  │
│  Access, docker CLI, tmutil, xcrun simctl    │
└─────────────────────────────────────────────┘
```

Zewnętrzne zależności runtime: tylko narzędzia już obecne u developera (`docker`, `tmutil`, `xcrun`). Brak usług chmurowych.

---

## 3. Stack technologiczny

### Język i toolchain
- **Rust (stable), edycja 2024**, target `aarch64-apple-darwin` (Intel poza zakresem).
- **Cargo workspace** z wieloma crate'ami.
- **Why**: szybkie skanowanie drzew plików, brak GC, jeden artefakt, a Rust jest dla agenta językiem o bardzo wysokiej biegłości.
- **Narzędzia**: `rustfmt`, `clippy` (warnings = errors w CI), `cargo-nextest` (testy), `cargo-deny` (licencje i podatności zależności, ważne przy MIT + Slint Royalty-free).

### UI
- **Slint** (aktualna stabilna wersja z linii 1.x, wersja przypięta w `Cargo.toml`), backend `winit`, renderer **Femtovg** (domyślny, lekki; Skia odrzucona w v1 jako cięższa).
- **Why**: deklaratywne, nowoczesne UI bez webview, natywny binarny build, a **oficjalny, wbudowany serwer MCP** pozwala agentowi sterować aplikacją (patrz sekcja „Narzędzia dla agenta AI”).
- **Licencja Slint**: **Royalty-free** (kod i wydania zostają MIT). Warunki: widżet `AboutSlint` w ekranie „O programie” dostępnym z menu oraz plakietka „Made with Slint” w README i na stronie wydań.
- **Styl**: wbudowany styl **Cupertino** (najbliższy wyglądowi macOS). Wystarczy „porządny, nowoczesny interfejs”, pełna natywność SwiftUI nie jest celem.
- **Języki interfejsu: polski i angielski** od v1. Teksty w `.slint` oznaczane mechanizmem tłumaczeń Slinta (`@tr()`, katalogi gettext `.po`), język domyślny wg ustawień systemu z możliwością przełączenia w ustawieniach. Wszystkie nowe teksty UI muszą od razu mieć oba warianty (reguła dla agenta w `AGENTS.md`). Opisy i uzasadnienia reguł bezpieczeństwa (np. „dlaczego to Ostrożnie”) też podlegają tłumaczeniu, więc trzymamy je jako klucze, a nie zaszyte w kodzie stringi.
- **Własne komponenty** (Slint ma mniej gotowych klocków niż ekosystem webowy):
  - tabela z sortowaniem i zaznaczaniem (wirtualizowana lista, bo wyników mogą być tysiące wierszy),
  - mapa dysku (treemap): układ liczony w `core` (algorytm squarified), rysowany w Slint jako model prostokątów,
  - dialogi potwierdzeń z odznakami statusu (Bezpieczne / Ostrożnie / Nie ruszać).

### Skaner i silnik reguł (`crates/core`)
- **`jwalk`** (równoległe przechodzenie po katalogach na `rayon`) lub `ignore`/`walkdir` + własna równoległość. Wybór po benchmarku na prawdziwym dysku w fazie 1.
- **Rozmiar = zajęte bloki na dysku (`st_blocks * 512`)**, a nie rozmiar logiczny pliku. Ma to znaczenie dla plików rzadkich (np. dysk wirtualny Dockera) i klonów APFS. Hardlinki i klony liczone raz.
- **`serde` + `serde_json`**: serializacja ustawień i wyników.
- **Reguły deterministyczne**: tabela reguł w kodzie (struktura danych, nie AI). Każda reguła ma: id, grupę narzędzia, dopasowanie ścieżki, status, uzasadnienie, sposób usunięcia (ścieżka lub polecenie). Ten sam wynik zawsze daje tę samą ocenę. Pozycje nierozpoznane pokazywane bez rekomendacji.
- **Wykrywanie `node_modules`** nieużywanych od tygodni: na podstawie dat dostępu/modyfikacji projektu (np. `package.json`, lockfile), z konfigurowalnym progiem.
- **Postęp na żywo**: skaner wysyła zdarzenia przez kanał (`crossbeam-channel` lub `std::sync::mpsc`) do wątku UI.

### Czyszczenie (`crates/cleaner`)
- **Kosz**: crate `trash` (natywne API macOS). Domyślna ścieżka usuwania.
- **Usuwanie bezpośrednie** (`std::fs::remove_dir_all`): wyłącznie dla pozycji „Bezpieczne”, po wyraźnej akcji użytkownika.
- **Docker**: wyłącznie przez CLI (`docker system df`, `docker ... prune`), uruchamiane przez `std::process::Command`, z podglądem dokładnego polecenia przed wykonaniem. Brak ingerencji w plik dysku wirtualnego. Aplikacja informuje, że plik może się nie skurczyć od razu.
- **Time Machine i symulatory**: `tmutil` (lokalne snapshoty) oraz `xcrun simctl` (np. usuwanie nieużywanych runtime'ów/urządzeń) jako adaptery poleceń.
- **Wszystkie efekty uboczne za trait'ami** (`FileSystem`, `CommandRunner`, `Trasher`). Dzięki temu da się je podmienić na atrapy w testach i w trybie testowym.

### Inwarianty bezpieczeństwa (obowiązkowe dla agenta)
1. Pozycja o statusie „Nie ruszać” **nie ma ścieżki usuwania w kodzie**, a nie tylko ukryty przycisk w UI.
2. Przed każdym usunięciem ścieżka jest **ponownie dopasowywana do reguły** (ochrona przed rozjazdem między skanem a usunięciem) i sprawdzana pod kątem symlinków wychodzących poza dozwolony obszar.
3. Progi 5 GB (pozycja „duża”) i 25 GB (podsumowanie zbiorcze) oraz ostrzeżenie dla operacji nieodwracalnych egzekwowane w warstwie `app`, konfigurowalne w ustawieniach.
4. Wolumeny Dockera i pozycje „Ostrożnie” domyślnie niezaznaczone.

### Dostęp do dysku (Full Disk Access)
- Aplikacja **nie jest w piaskownicy** (powód, dla którego Mac App Store odpada) i działa z hardened runtime.
- Brak możliwości programowego nadania uprawnienia, więc onboarding wykrywa jego brak (próba odczytu chronionej lokalizacji), tłumaczy użytkownikowi, po co jest potrzebne, i otwiera odpowiedni panel w Ustawieniach systemu.

### Ustawienia i dane lokalne
- **Jeden plik JSON** w katalogu aplikacji (crate `directories`): progi, preferencje. Bez bazy danych, kont i telemetrii.

### Błędy i logowanie
- `thiserror` (błędy w bibliotekach), `anyhow` (w warstwie `app`), `tracing` do logów lokalnych (bez wysyłania gdziekolwiek).

### Narzędzia dla agenta AI (development i testy)

**Warstwa 1: testy logiki bez UI (podstawa).**
- `cargo nextest` + `tempfile`: skaner, reguły i czyszczenie testowane na sztucznych drzewach katalogów.
- `proptest` dla niezmienników (np. „nic o statusie Nie ruszać nigdy nie trafia do listy usuwanych”).
- Atrapy `CommandRunner` dla Dockera, `tmutil` i `simctl` (bez uruchamiania prawdziwych poleceń).
- Opcjonalnie `insta` (snapshoty) dla wyników skanu i podsumowań.

**Warstwa 2: sterowanie działającą aplikacją przez agenta (Claude Code).**
- **Wbudowany serwer MCP Slinta** (testing backend): drzewo UI, zrzuty ekranu, klikanie, przeciąganie, pisanie. Włączany wyłącznie w buildach deweloperskich: feature `slint/mcp`, zmienne `SLINT_EMIT_DEBUG_INFO=1` i `SLINT_MCP_PORT=<port>`. W projekcie powstanie alias (np. `cargo xtask dev-mcp`) i wpis w `.mcp.json`, żeby agent uruchamiał to jednym poleceniem.
- Feature `mcp` **nigdy** nie jest włączany w buildach wydaniowych (kontrola w CI).
- **API testowe Slinta** (`i-slint-backend-testing`) do testów komponentów UI w trybie headless, bez okna.
- **Plan awaryjny**: `agent-desktop` (CLI w Rust, API dostępności macOS, z trybem MCP), jeśli MCP Slinta okaże się niewystarczające lub niestabilne (jego API jest oficjalnie oznaczone jako wstępne).

**Tryb testowy (krytyczne dla narzędzia kasującego pliki).**
- Zmienna środowiskowa (np. `FDC_TEST_ROOT=<katalog>`) przełącza aplikację w tryb, w którym skanuje i czyści **wyłącznie** wskazane sztuczne drzewo (fikstury), a polecenia Dockera, `tmutil` i `simctl` idą przez atrapy.
- W tym trybie okno ma widoczny baner „TRYB TESTOWY”. Agent **nigdy** nie testuje czyszczenia na prawdziwym dysku.
- Generator fikstur (`cargo xtask fixtures`) tworzy drzewo imitujące DerivedData, `node_modules`, cache yarn/npm, wolumeny itd., z kontrolowanymi rozmiarami.

**Skill i kontekst dla agenta.**
- `AGENTS.md`/`CLAUDE.md` z regułami projektu, w tym inwariantami bezpieczeństwa i zakazem testowania na prawdziwym dysku (do wygenerowania osobnym skillem po zatwierdzeniu TSD).

### Analityka
- **Brak.** Zero telemetrii (wymóg PRD).

---

## 4. Dystrybucja i CI/CD

| Element | Rozwiązanie | Koszt |
|---|---|---|
| Repozytorium i wydania | GitHub (publiczne), GitHub Releases | 0 |
| CI | GitHub Actions, runner macOS arm64 | 0 dla repo publicznego (limity wg GitHub) |
| Podpisywanie | Certyfikat **Developer ID Application** (już posiadany) | w ramach konta Apple Developer (99 USD/rok) |
| Notaryzacja | `xcrun notarytool` + `stapler` | w ramach konta |
| Pakowanie | `.app` + `.dmg` (narzędzie pakujące do wyboru w fazie 1, patrz Otwarte pytania) | 0 |

**Pipeline:**
- **Każdy PR**: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest`, `cargo deny check`, sprawdzenie, że feature `mcp` nie trafia do buildu wydaniowego.
- **Tag wersji (`vX.Y.Z`)**: build release (arm64) → złożenie `.app` → podpis Developer ID z hardened runtime → notaryzacja i `stapler` → `.dmg` → publikacja w GitHub Releases.
- **Sekrety w GitHub Actions**: certyfikat Developer ID `.p12` (base64) z hasłem oraz **klucz API App Store Connect** (plik `.p8`, Key ID i Issuer ID) przekazywany do `notarytool`. Certyfikat importowany do tymczasowego keychaina na runnerze i usuwany po buildzie, a klucz `.p8` zapisywany tylko na czas kroku notaryzacji. Żaden z sekretów nie jest dostępny dla workflow uruchamianych z forków.
- **Wymagana strona wydań/README**: plakietka „Made with Slint” (warunek licencji Royalty-free).

---

## 5. Struktura projektu

```
freediskcleaner/
├── Cargo.toml                 # workspace
├── crates/
│   ├── core/                  # skaner, model, silnik reguł, ocena, układ treemap (bez zależności od UI)
│   │   ├── src/
│   │   └── tests/
│   ├── cleaner/               # Kosz, usuwanie, adaptery Docker/tmutil/simctl (za trait'ami)
│   └── app/                   # binarka: stan aplikacji, wątki, kanały, spięcie ze Slint
│       ├── src/
│       ├── ui/                # pliki .slint (widoki, komponenty, motyw)
│       └── translations/      # katalogi tłumaczeń PL/EN (.po)
├── xtask/                     # cargo xtask: fixtures, dev-mcp, package, release
├── fixtures/                  # definicje sztucznych drzew do testów
├── packaging/                 # Info.plist, ikona, entitlements, skrypty DMG
├── .github/workflows/         # ci.yml, release.yml
├── .mcp.json                  # rejestracja MCP Slinta dla Claude Code (tylko dev)
├── AGENTS.md / CLAUDE.md      # kontekst i reguły dla agentów
├── deny.toml                  # polityka licencji i podatności
├── LICENSE                    # MIT
└── README.md                  # w tym plakietka „Made with Slint”
```

---

## 6. Kluczowe decyzje techniczne

1. **Rust + Slint, bez webview** — jeden język, jeden binarny plik, nowoczesne UI i oficjalne narzędzie MCP do sterowania aplikacją przez agenta. Kosztem jest mniejszy ekosystem gotowych komponentów (mapę dysku i tabelę budujemy sami).
2. **Rdzeń odseparowany od UI (`core` + `cleaner`)** — cała logika oceny i usuwania testowalna bez okna i bez AI. UI jest cienką warstwą.
3. **Oceny bezpieczeństwa jako deterministyczna tabela reguł** — bez AI w v1 (zgodnie z PRD). Nierozpoznane = brak rekomendacji.
4. **„Nie ruszać” egzekwowane w kodzie, nie w UI** — brak ścieżki usuwania dla takich pozycji, plus ponowna weryfikacja reguły tuż przed usunięciem.
5. **Rozmiar liczony z zajętych bloków** — poprawne wartości dla plików rzadkich (Docker) i klonów APFS.
6. **Efekty uboczne za trait'ami + tryb testowy na fiksturach** — agent może testować pełny przepływ (skan → zaznaczenie → potwierdzenie → czyszczenie) bez ryzyka dla prawdziwych danych.
7. **Docker tylko przez CLI** — nigdy przez kasowanie pliku dysku wirtualnego (opcja poza zakresem v1).
8. **MCP Slinta wyłącznie w buildach deweloperskich** — kontrolowane w CI. Plan awaryjny: `agent-desktop`.
9. **Slint Royalty-free** — zachowuje licencję MIT dla kodu i wydań, w zamian za atrybucję (About + plakietka).
10. **Brak piaskownicy, Full Disk Access w onboardingu** — konieczne dla czyszczenia, stąd dystrybucja poza Mac App Store.
11. **Podpis i notaryzacja w GitHub Actions** — wydania bez ostrzeżeń systemowych dla znajomych.

---

## 7. Poza zakresem (na razie)

- Analizator AI wyjaśniający nierozpoznane foldery (v2).
- Usuwanie całego dysku wirtualnego Dockera.
- Alternatywy dla Docker Desktop (OrbStack, Colima i inne).
- Maki z procesorem Intel, Windows, Linux, wersje mobilne.
- Mac App Store i piaskownica.
- Automatyczne czyszczenie w tle i harmonogramy.
- Wyszukiwanie duplikatów, antywirus, optymalizacja RAM, odinstalowywanie aplikacji.
- Konta, synchronizacja, telemetria.
- Automatyczne aktualizacje aplikacji (w v1 aktualizacja przez pobranie nowego wydania).
- Dodatkowe języki interfejsu poza polskim i angielskim.

---

## 8. Otwarte pytania

1. **Narzędzie do pakowania `.app`/`.dmg`**: gotowy packager (np. `cargo-packager`/`cargo-bundle`) czy własny skrypt w `xtask`. Do zdecydowania po prototypie w fazie 1.
2. **Dojrzałość MCP Slinta**: pierwsze zadanie fazy 1 to spike weryfikujący, że agent potrafi uruchomić aplikację, kliknąć przycisk „Skanuj” i zrobić zrzut ekranu na macOS. Wynik decyduje, czy plan awaryjny (`agent-desktop`) będzie potrzebny.
3. **Biblioteka skanowania**: `jwalk` czy własna równoległość na `rayon`, po benchmarku na prawdziwym dysku.
4. **Reguły „danych systemowych”** (z PRD): które lokalizacje trafiają do „krytycznych” (Nie ruszać), a które do „do usunięcia” (Ostrożnie). Lista do ustalenia na etapie projektu technicznego, wraz z testami opartymi na fiksturach.
5. **Minimalna wersja macOS** do wsparcia (wpływa na API systemowe i konfigurację targetu).

### Rozstrzygnięte

- **Styl UI**: Cupertino.
- **Języki interfejsu**: polski i angielski od v1.
- **Notaryzacja w CI**: klucz API App Store Connect.
