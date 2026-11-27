> **Language note: the English changelog comes first.** It ends at the `---`
> separator, after which the same changelog is repeated in Russian.
> From 0.9.3 on, every WinRust release is published in both languages:
> English first, then Russian.

Windows 10/11 system components rewritten in Rust. Release 0.9.6 restores the
window switcher the shell took with it, and leaves the tree clean.

## 1. Alt+Tab

* **The switcher is back, and it is ours.** Windows keeps `Alt+Tab` inside
  `explorer.exe`, so replacing the shell removed it: the chord did nothing at
  all. It is a module of its own now.
* **How it behaves.** Hold `Alt`, tap `Tab` - a panel lists every open window
  with its picture, most recently used first. `Tab` walks the selection
  forward, `Shift+Tab` walks it back, and releasing `Alt` raises the window you
  stopped on. A card can also be clicked to switch straight to that window, and
  `Esc` closes the panel without switching. The panel is only up while `Alt` is
  held, exactly like the stock one.
* **Why it is polled, not hooked.** `Alt+Tab` is a reserved shell chord: the
  system takes it before any window receives a key event. The module reads the
  key state with `GetAsyncKeyState`, the same way `Win+V` and `Win+I` already
  work from a window that does not have the focus.

## 2. The two things a switcher gets wrong

* **The order.** `EnumWindows` hands windows back in z-order, *oldest first*,
  so the front-most window arrives last. The list is reversed, which is what
  makes one tap of `Tab` mean "go back where I was" instead of "go to the
  window I have not touched in an hour".
* **The press that opens the panel.** A single `Tab` down was doing two jobs:
  it opened the panel *and* stepped the selection, so the first card the user
  ever saw was already the second window. The opening press is consumed by the
  opening itself.

## 3. Seeing the panel at all

* **The shell window had to be lifted.** The desktop window is
  `always_on_bottom` by design - it *is* the desktop, so a normal window would
  hide the icons and the taskbar it hosts. A panel drawn in it would have been
  covered by the very windows it switches between. For as long as the panel is
  up the window is raised to `AlwaysOnTop`, and it drops back to
  `AlwaysOnBottom` when the panel closes.
* **Pictures are taken once, on open.** A live refresh was measured against its
  cost: `PrintWindow` on every window in the list is more than a panel that
  lives for half a second is worth. A window that disappears loses its entry in
  the cache, so it cannot grow for the life of the shell.

## 4. Where the new state lives

| new in 0.9.6 | owner |
| --- | --- |
| the panel, the order and the selection | `alttab::AltTab` |
| whether the shell window is currently raised | `AltTab::raised` |
| the pictures of the candidate windows | `DesktopApp::thumbs`, shared with the taskbar |

The panel's state is one value with verbs, not three loose fields: a selection
without the panel is nothing, and an order without a selection cannot be
walked. Four of the module's nine methods are private to it.

## 5. The rest of the tree

* **A deslop pass over every crate, finding nothing to fix.** No file is over
  2000 lines, no directory holds more than fifteen modules, and there are no
  module cycles or three-node tangles anywhere in the workspace.
* **The new module was held to the same rule.** It came out of the pass with
  five of its nine methods public - more than the rest of the shell exposes
  together. The ones that only serve the module's own panel are private now,
  and the colour helpers became free functions instead of a trait with an empty
  body, which is the shape the linter rejects.

`cargo fmt --all`, `cargo build` and `cargo clippy` are clean on every crate;
the release, build, fmt, clippy and test jobs are green on the tagged commit.

---

> **Примечание о языке: ниже та же самая версия списка изменений на русском.**
> Начиная с 0.9.3 заметки к каждому релизу WinRust публикуются на двух языках:
> сначала английский, затем русский. Английская часть заканчивается
> разделителем `---` выше.

Компоненты Windows 10/11, переписанные на Rust. Релиз 0.9.6 возвращает
переключатель окон, который унесла с собой замена оболочки, и оставляет дерево
чистым.

## 1. Alt+Tab

* **Переключатель вернулся, и он наш.** Windows держит `Alt+Tab` внутри
  `explorer.exe`, поэтому замена оболочки его убрала: сочетание не делало
  ровно ничего. Теперь это отдельный модуль.
* **Как он себя ведёт.** Зажмите `Alt`, нажмите `Tab` — появится панель со
  всеми открытыми окнами и их снимками, последние использованные первыми.
  `Tab` ведёт выделение вперёд, `Shift+Tab` назад, отпускание `Alt` поднимает
  то окно, на котором вы остановились. По карточке можно щёлкнуть, чтобы
  переключиться сразу на это окно, а `Esc` закрывает панель без переключения.
  Панель видна только пока зажат `Alt` — ровно как штатная.
* **Почему опрос, а не хук.** `Alt+Tab` — зарезервированное системное
  сочетание: система забирает его раньше, чем какое-либо окно получит событие
  клавиши. Модуль читает состояние клавиш через `GetAsyncKeyState` — так же,
  как уже работают `Win+V` и `Win+I` из окна без фокуса.

## 2. Две вещи, которые переключатель делает не так

* **Порядок.** `EnumWindows` возвращает окна в z-порядке, *старейшие первыми*,
  поэтому переднее окно приходит последним. Список разворачивается — именно
  это делает одно нажатие `Tab` означающим «верни меня туда, где я был», а не
  «перейди к окну, которое я не трогал час».
* **Нажатие, открывающее панель.** Одно нажатие `Tab` делало две работы:
  открывало панель *и* сдвигало выделение, поэтому первая же увиденная
  карточка была уже вторым окном. Нажатие, открывшее панель, поглощается
  самим открытием.

## 3. Как эту панель вообще увидеть

* **Окно оболочки пришлось поднять.** Окно рабочего стола по замыслу
  `always_on_bottom` — это и *есть* рабочий стол, поэтому обычное окно скрыло
  бы значки и панель задач, которые оно содержит. Панель, нарисованная в нём,
  была бы закрыта теми самыми окнами, между которыми она переключает. Пока
  панель видна, окно поднимается в `AlwaysOnTop`, а при её закрытии
  возвращается в `AlwaysOnBottom`.
* **Снимки делаются один раз, при открытии.** Живое обновление было сверено со
  своей ценой: `PrintWindow` для каждого окна из списка — больше, чем стоит
  панель, живущая полсекунды. Исчезнувшее окно теряет свою запись в кэше,
  поэтому он не может расти всё время жизни оболочки.

## 4. Где живёт новое состояние

| новое в 0.9.6 | владелец |
| --- | --- |
| панель, порядок и выделение | `alttab::AltTab` |
| поднято ли сейчас окно оболочки | `AltTab::raised` |
| снимки окон-кандидатов | `DesktopApp::thumbs`, общий с панелью задач |

Состояние панели — одно значение с глаголами, а не три отдельных поля:
выделение без панели — ничто, а по порядку без выделения нельзя идти. Четыре
из девяти методов модуля приватны для него.

## 5. Остальное дерево

* **Прогон deslop по всем крейтам, и чинить оказалось нечего.** Ни один файл
  не длиннее 2000 строк, ни один каталог не держит больше пятнадцати модулей,
  циклов и трёхузловых запутанностей по всему рабочему пространству нет.
* **Новый модуль держали по тому же правилу.** Из прогона он вышел с пятью
  публичными методами из девяти — больше, чем остальная оболочка выставляет
  вместе. Те, что нужны только собственной панели модуля, стали приватными, а
  цветовые хелперы стали свободными функциями вместо трейта с пустым телом —
  именно эту форму линтер и отвергает.

`cargo fmt --all`, `cargo build` и `cargo clippy` чисты на каждом крейте;
задания release, build, fmt, clippy и test зелёные на отмеченном коммите.
