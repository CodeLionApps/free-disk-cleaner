# FreeDiskCleaner

*Darmowy cleaner dysku dla developerów na macOS: pokazuje, co zajmuje miejsce, i pozwala to bezpiecznie usunąć.*

## 1. Przegląd produktu

FreeDiskCleaner to darmowa, otwartoźródłowa (licencja MIT) aplikacja okienkowa na macOS z Apple Silicon. Skanuje dysk, rozpoznaje, co na nim zalega (cache'e Xcode, symulatory iOS i Androida, obrazy i dane Dockera, node_modules, cache yarn/npm, Homebrew i inne), i pozwala to bezpiecznie usunąć. Pokazuje dokładne ścieżki i rozmiary, grupuje wyniki według narzędzi i przy każdej pozycji mówi, czy wolno ją ruszać. Czyszczenie nie jest za paywallem: zeskanujesz, zrozumiesz i usuniesz w jednej aplikacji.

## 2. Problem

Developer mobilny i backendowy na MacBooku zapycha dysk systematycznie: DerivedData i archiwa Xcode, stare runtime'y symulatorów, obrazy i wolumeny Dockera, dziesiątki `node_modules`, cache menedżerów paczek, Gradle i Homebrew. Po kilku miesiącach 500 GB dysku jest zajęte w 98%, a kategoria "Dane systemowe" pochłania ćwierć terabajta i nie mówi nic o tym, co w niej jest.

Istniejące narzędzia zawodzą na dwa sposoby:

- Płatne cleanery (np. CleanMyMac) skanują za darmo, ale samo czyszczenie wymaga licencji, a wyniki często nie wskazują konkretnych ścieżek.
- Wizualizatory (np. DaisyDisk) pokazują, gdzie jest miejsce, ale nie wiedzą, co jest bezpieczne do usunięcia, więc użytkownik zostaje z mapą i obawą, że coś zepsuje.

## 3. Grupa docelowa

Główna persona: developer pracujący na macOS (iOS, Android, backend, Docker, JS/TS), który używa yarn lub npm po kilka razy dziennie na wielu projektach i regularnie dobija do limitu dysku. Zna terminal, ale nie chce pamiętać kilkunastu ścieżek i komend `rm` ani ryzykować usunięciem czegoś ważnego.

V1 powstaje przede wszystkim na własny użytek autora, a następnie do podzielenia się ze znajomymi developerami. Obsługiwane są wyłącznie Maki z Apple Silicon.

## 4. Propozycja wartości i pozycjonowanie

Przewaga to połączenie trzech rzeczy, których żadne darmowe narzędzie nie daje razem: wiedza o tym, co jest developerskimi śmieciami, a co systemem, dokładne ścieżki z wyjaśnieniem każdej pozycji oraz realne czyszczenie bez płacenia.

> Dla developerów na macOS, którym kończy się miejsce na dysku, FreeDiskCleaner jest darmową aplikacją do czyszczenia, która pokazuje dokładnie, co zajmuje miejsce, ocenia, czy można to usunąć, i usuwa to za Ciebie, w przeciwieństwie do CleanMyMac, który blokuje czyszczenie za licencją, i DaisyDisk, który pokazuje dane, ale nie wie, co jest bezpieczne.

## 5. Funkcje v1

### Skanowanie i widoczność

- Jednym kliknięciem skanuję dysk i widzę, na co idzie miejsce, w rozsądnym czasie i z postępem na żywo.
- Widzę rozbicie "danych systemowych" na konkretne składniki (np. lokalne snapshoty Time Machine, dane Dockera, cache'e systemowe), zamiast jednej tajemniczej liczby.
- Każda pozycja ma pełną ścieżkę, rozmiar, datę ostatniego użycia i krótki opis, czym jest.

### Grupowanie

- Wyniki są pogrupowane według narzędzi: Xcode i iOS, Android, Docker, Node/JS, Homebrew i inne, aplikacje i system.
- Widok tabeli lub mapy dysku pozwala zejść od grupy do konkretnego folderu.
- Mogę znaleźć projekty, w których node_modules leży nieużywane od wielu tygodni, i usunąć je hurtem (zawartość odtworzy się przez yarn/npm).

### Ocena bezpieczeństwa

- Każda pozycja ma jeden z trzech statusów z uzasadnieniem: Bezpieczne (odtwarzalny cache), Ostrożnie (można stracić dane, np. wolumeny Dockera czy dane aplikacji w symulatorze), Nie ruszać (system i pliki krytyczne).
- Pozycje oznaczone "Nie ruszać" nie mają przycisku usuwania, nigdy.
- Ocena opiera się wyłącznie na regułach deterministycznych dla znanych lokalizacji, więc ten sam wynik zawsze daje tę samą ocenę. Pozycje nierozpoznane są pokazane z rozmiarem i ścieżką, ale bez rekomendacji.

### Czyszczenie

- Zaznaczam pozycje i usuwam je z poziomu aplikacji. Domyślnie trafiają do Kosza, żeby dało się je cofnąć.
- Dla pozycji "Bezpieczne" (odtwarzalne cache'e) jest opcja "usuń od razu, bez Kosza", bo przeniesienie do Kosza nie zwalnia miejsca, dopóki Kosz nie zostanie opróżniony.
- Przed usunięciem widzę podsumowanie: co dokładnie zniknie i ile miejsca odzyskam. Po usunięciu widzę, ile faktycznie udało się odzyskać.

### Ostrzeżenia przy dużych i nieodwracalnych operacjach

- Pozycja większa niż 5 GB jest oznaczona jako "duża" i wymaga dodatkowego potwierdzenia.
- Zaznaczenie łącznie powyżej 25 GB w jednym czyszczeniu pokazuje ekran podsumowania z wyraźnym ostrzeżeniem.
- Każda operacja, której nie da się cofnąć (usuwanie bez Kosza, polecenia Dockera), wymaga ostrzeżenia niezależnie od rozmiaru.
- Oba progi (5 GB i 25 GB) są domyślne i można je zmienić w ustawieniach.

### Docker

- Aplikacja pyta Dockera o zajęte miejsce (obrazy, kontenery, wolumeny, build cache) i pokazuje je jako osobne pozycje w grupie Docker. Wymaga uruchomionego Dockera.
- Czyszczenie odbywa się wyłącznie przez polecenia Dockera, nie przez kasowanie plików dysku wirtualnego. Przed uruchomieniem widzę dokładnie, jakie polecenie zostanie wykonane.
- Oceny: build cache i osierocone obrazy to Bezpieczne; nieużywane obrazy i zatrzymane kontenery to Ostrożnie (trzeba będzie ściągnąć ponownie); wolumeny to Ostrożnie i domyślnie są niezaznaczone, bo mogą trzymać dane lokalnych baz.
- Po czyszczeniu plik dysku wirtualnego nie zawsze od razu się kurczy. Aplikacja uczciwie o tym informuje, zamiast obiecywać odzyskaną przestrzeń, której jeszcze nie widać na dysku.

### Dane systemowe

- "Dane systemowe" nie są jedną pozycją, tylko dwiema osobnymi kategoriami, w zależności od tego, czy faktycznie można je usunąć.
- Dane systemowe krytyczne (czerwony wykrzyknik): pliki i foldery, których usunięcie grozi uszkodzeniem systemu. Mają status Nie ruszać, są pokazane tylko informacyjnie, z rozmiarem i wyjaśnieniem, i nie mają przycisku usuwania.
- Dane systemowe do usunięcia (żółty znak ostrzegawczy): dane tworzone przez system, które można bezpiecznie zredukować, np. stare lokalne snapshoty Time Machine, gdy jest ich wiele, albo nieużywane stare runtime'y. Mają status Ostrożnie, domyślnie niezaznaczone, z wyjaśnieniem, co dokładnie zostanie usunięte i co stracę.
- Przy takich pozycjach aplikacja podpowiada, co można zostawić (np. najnowszy backup), a co usunąć (starsze).

### Instalacja dla znajomych

- Pobieram podpisaną i notaryzowaną aplikację z GitHub Releases, otwieram, przechodzę krótki onboarding (w tym przyznanie wymaganego dostępu do dysku) i skanuję. Bez konta, bez konfiguracji.

## 6. Poza zakresem v1

- Analizator AI wyjaśniający nierozpoznane foldery (planowany na v2).
- Usuwanie całego dysku wirtualnego Dockera (opcja "ostatniej szansy", ewentualnie v2).
- Alternatywy dla Docker Desktop (OrbStack, Colima i inne).
- Maki z procesorem Intel, Windows, Linux i wersje mobilne.
- Dystrybucja przez Mac App Store (wymagana piaskownica uniemożliwia dostęp do plików, które mamy czyścić).
- Automatyczne czyszczenie w tle i harmonogramy.
- Wyszukiwanie duplikatów plików i zdjęć.
- Antywirus, optymalizacja RAM, monitoring systemu.
- Odinstalowywanie aplikacji wraz z resztkami.
- Konta użytkowników, synchronizacja, telemetria.

## 7. Dystrybucja, licencja i cena

Aplikacja jest w pełni darmowa, bez reklam, kont i ukrytych limitów, na licencji MIT. Kod jest otwarty na GitHubie. Gotowe wydania (GitHub Releases) podpisuje certyfikatem Developer ID i notaryzuje autor, dzięki czemu znajomi uruchamiają aplikację bez ostrzeżeń systemowych. Osoby budujące ze źródeł podpisują własną kopię same. App Store nie jest kanałem dystrybucji (patrz zakres).

Naturalne kanały promocji, jeśli projekt się przyjmie: wzmianki w społecznościach developerskich (r/macapps, r/iOSProgramming, Hacker News).

## 8. Otwarte pytania

- Zakres v2 do ustalenia po v1: m.in. analizator AI (lokalny model czy API z własnym kluczem, co wolno wysyłać), alternatywy dla Docker Desktop i opcja usuwania całego dysku wirtualnego Dockera.
- Jakie konkretnie lokalizacje trafiają do kategorii "dane systemowe do usunięcia" a jakie do "krytycznych"? Lista reguł do ustalenia na etapie projektu technicznego.

