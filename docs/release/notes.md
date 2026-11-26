> **Language note: the English changelog comes first.** It ends at the `---`
> separator, after which the same changelog is repeated in Russian.
> From 0.9.3 on, every WinRust release is published in both languages:
> English first, then Russian.

Windows 10/11 system components rewritten in Rust. Release 0.9.4 answers a
fourth review: ten reports against 0.9.3, fixed at their causes, plus the
structural work the review asked for before any of the fixes went in.

## 1. Start menu

* **The footer buttons have their frames back.** The theme set a fill for
  the inactive, hovered and active widget states but only ever a stroke for
  `noninteractive`, so an ordinary button drew as frameless text on the
  panel. All three states now carry an outline - the same rule every window
  in the shell follows, so this was fixed in `winrust-ui::theme`, not by
  patching the one menu that showed it.
* **The menu is anchored to the left edge**, eight pixels in, at the height
  it already had. It was centred.
* **Pinned tiles can be dragged onto each other** to reorder the grid, and
  the order is saved. A drag spans frames, so the tile being dragged is held
  in egui's own memory rather than in the per-frame output struct; the tile
  under the pointer is outlined while the drag is live.

## 2. Taskbar

* **Buttons group by program.** The key is the lower-cased executable name
  (`pid:<n>` when the name is not available), so three Explorer windows are
  one button, exactly as screen 9 of the review showed.
* **A group lights up while one of its windows has the focus**, and the
  thumbnail preview lists the whole group instead of drawing a separate
  button per window.
* The volume slider was still stuck at 50 %. The real cause was the device
  enumerator: `CoCreateInstance` was called with the wrong CLSID and the
  wrong IID, so it failed before any `Activate` call and the slider never
  saw a device. Both GUIDs are corrected -
  `BCDE0395-E52F-467C-8E3D-C4579291692E` and
  `A95664D2-9614-4F35-A746-DE8DB63617E6`. Neither reading nor setting the
  level needs elevation.

## 3. Desktop

* **A selected icon shows its whole name.** The caption was capped at two
  short lines even while the icon was selected, which is when the name
  matters most. The clip rectangle widens for the selected icon and returns
  to the cell when the selection is cleared by a click on empty space.

## 4. Explorer

* **Any left press that does not land on a row clears the selection** - the
  left pane, the command bar, the gap between rows, the empty strip. It does
  not fire on the click that made the selection, because it only clears when
  the selection is unchanged since the frame began.
* **Left-dragging no longer scrolls the file views.** `drag_to_scroll` was
  on, so a drag that started between rows panned the list instead of drawing
  a band. Wheel scrolling is untouched, and the four views (details, list,
  tiles, content) all opt out.
* **A gutter down the left edge** gives a rubber band a place to start in a
  long folder: rows begin at the pane's edge, so before this there was no
  pixel to press that was not on a row.
* **New "Icon-only drag image" switch** in the background View menu - the
  second variant from the screenshot. It persists next to the other Explorer
  preferences.
* **Favourite folders can be reordered** by dragging one row of the left
  pane's list onto another; the saved order is the order you chose.

## 5. Structure

The review asked for the deslop findings to be fixed from first principles
before the bug fixes, with a decision about which module belongs at which
layer. `docs/structure-0.9.4.md` holds the before/after diagram.

* **The `session` <-> `start` cycle is gone.** The two modules referred to
  each other: the view wanted two types from the state owner, the state
  owner wanted one function from the view. The shared vocabulary
  (`AppEntry`, `PowerAction`, `default_pins`) moved to a new bottom layer,
  `model`, which both depend on downwards.
* **`Session` was three things in one.** The `Win+V` panel is now
  `ClipboardPanel`; each of the two chords is a `Chord`, and the "already
  seen" latch is the type, so a caller cannot forget to reset it.
* **Two public fields became verbs.** `power_menu_open` and
  `user_menu_open` made a round trip through the per-frame output and were
  never read as state. They are private now, and toggling one closes the
  other - an invariant the public field let callers break.
* **`DesktopApp::selected` was deleted.** It was written in eight places and
  read in none: a second copy of `selection`.
* **`ExplorerApp`'s eight channel fields are one `Workers`.** They were
  `rx`/`tx`/`job_rx`/`job_tx`/`search_rx`/`search_tx`/`folder_size_rx`/
  `folder_size_tx`, all public, and all only touched by `pump_workers` and
  the two spawn sites. The shell can now hand out a sender or drain a
  receiver, and cannot see a channel.

## Where the new state lives

| new in 0.9.4 | owner |
| --- | --- |
| the order of the pinned tiles | `model::default_pins`, `session::Session::move_pin` |
| who owns the in-flight pin drag | egui memory, `Id::new("start_pin_drag")` |
| which drag image Explorer draws | `PersistedState::minimal_drag_image` |
| the order of the favourite folders | `ExplorerApp::pinned` |
| the four background queues | `app::state::Workers` |
| the Start menu's shared types | `model` (new bottom layer) |

`cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
build, fmt, clippy and test jobs are green on the tagged commit.

---

> **Примечание о языке: ниже та же самая версия списка изменений на русском.**
> Начиная с 0.9.3 заметки к каждому релизу WinRust публикуются на двух языках:
> сначала английский, затем русский. Английская часть заканчивается
> разделителем `---` выше.

Компоненты Windows 10/11, переписанные на Rust. Релиз 0.9.4 отвечает на
четвёртое ревью: десять замечаний по 0.9.3, исправленных по причинам, плюс
структурная работа, которую ревью требовало сделать до самих исправлений.

## 1. Меню «Пуск»

* **У нижних кнопок вернулись рамки.** Тема задавала заливку для состояний
  `inactive`, `hovered` и `active`, а обводку — только для `noninteractive`,
  поэтому обычная кнопка рисовалась как текст без рамки прямо на панели.
  Теперь обводка есть у всех трёх состояний. Правило общее для всех окон
  оболочки, поэтому исправлено в `winrust-ui::theme`, а не заплаткой в одном
  меню, где это было видно.
* **Меню прижато к левому краю** с отступом восемь пикселей, высота прежняя.
  Раньше оно стояло по центру.
* **Плитки закреплённых приложений перетаскиваются друг на друга** и меняют
  порядок в сетке; порядок сохраняется. Перетаскивание длится несколько
  кадров, поэтому перетаскиваемая плитка хранится в памяти egui, а не в
  структуре, которая пересобирается каждый кадр; плитка под курсором
  обводится, пока идёт перетаскивание.

## 2. Панель задач

* **Кнопки группируются по программе.** Ключ — имя исполняемого файла в
  нижнем регистре (`pid:<n>`, если имени нет), поэтому три окна проводника —
  это одна кнопка, ровно как на девятом снимке ревью.
* **Группа подсвечивается, пока фокус у одного из её окон**, а предпросмотр
  перечисляет всю группу, а не рисует отдельную кнопку на каждое окно.
* Ползунок громкости всё ещё застревал на 50 %. Настоящая причина —
  перечислитель устройств: `CoCreateInstance` вызывался с неверным CLSID и
  неверным IID, поэтому падал ещё до вызова `Activate`, и ползунок не видел
  устройство. Оба GUID исправлены —
  `BCDE0395-E52F-467C-8E3D-C4579291692E` и
  `A95664D2-9614-4F35-A746-DE8DB63617E6`. Ни чтение, ни установка уровня
  прав администратора не требуют.

## 3. Рабочий стол

* **Выделенный значок показывает имя целиком.** Подпись обрезалась до двух
  коротких строк даже у выделенного значка, а именно тогда имя и важно.
  Область обрезки расширяется для выделенного значка и возвращается к ячейке,
  когда выделение снимается щелчком по пустому месту.

## 4. Проводник

* **Любое нажатие левой кнопки мимо строки снимает выделение** — левая
  панель, панель команд, промежуток между строками, пустая полоса. Оно не
  срабатывает на щелчке, которым выделение и создано: снимается только
  тогда, когда выделение не изменилось с начала кадра.
* **Перетаскивание левой кнопкой больше не прокручивает список файлов.**
  `drag_to_scroll` был включён, поэтому перетаскивание, начатое между
  строками, прокручивало список вместо рамки выделения. Прокрутка колесом не
  затронута; отказ включают все четыре вида (таблица, список, плитки,
  содержимое).
* **Полоса вдоль левого края** даёт рамке выделения место для старта в длинной
  папке: строки начинаются от края панели, поэтому раньше не было пикселя,
  нажатие на который не попадало бы в строку.
* **Новый переключатель «Перетаскивать только значок»** в меню «Вид» фонового
  контекстного меню — второй вариант со снимка. Он сохраняется рядом с
  остальными настройками проводника.
* **Папки в «Избранном» можно переставлять**, перетаскивая одну строку списка
  левой панели на другую; сохранённый порядок — это выбранный вами порядок.

## 5. Структура

Ревью требовало исправить находки deslop по причинам, а не по симптомам, и до
исправления ошибок — с решением, какой модуль к какому слою относится.
Диаграмма «до/после» лежит в `docs/structure-0.9.4.md`.

* **Цикл `session` <-> `start` разорван.** Два модуля ссылались друг на
  друга: вид брал два типа у владельца состояния, владелец состояния — одну
  функцию у вида. Общий словарь (`AppEntry`, `PowerAction`,
  `default_pins`) переехал в новый нижний слой `model`, от которого оба
  зависят сверху вниз.
* **`Session` был тремя вещами в одной.** Панель `Win+V` теперь
  `ClipboardPanel`; каждое из двух сочетаний клавиш — `Chord`, и защёлка
  «уже видели» является типом, поэтому забыть её сбросить нельзя.
* **Два публичных поля стали глаголами.** `power_menu_open` и
  `user_menu_open` ходили туда-обратно через вывод кадра и ни разу не
  читались как состояние. Теперь они приватные, и открытие одного закрывает
  другое — инвариант, который публичное поле позволяло нарушить.
* **`DesktopApp::selected` удалено.** Оно записывалось в восьми местах и не
  читалось ни в одном: вторая копия `selection`.
* **Восемь полей-каналов `ExplorerApp` — один `Workers`.** Это были
  `rx`/`tx`/`job_rx`/`job_tx`/`search_rx`/`search_tx`/`folder_size_rx`/
  `folder_size_tx`, все публичные, и все нужные только `pump_workers` и двум
  местам запуска потоков. Теперь оболочка может выдать отправителя или
  опустошить получателя, но не увидеть канал.

## Где живёт новое состояние

| новое в 0.9.4 | владелец |
| --- | --- |
| порядок закреплённых плиток | `model::default_pins`, `session::Session::move_pin` |
| кто владеет текущим перетаскиванием плитки | память egui, `Id::new("start_pin_drag")` |
| какой образ перетаскивания рисует проводник | `PersistedState::minimal_drag_image` |
| порядок папок в «Избранном» | `ExplorerApp::pinned` |
| четыре фоновые очереди | `app::state::Workers` |
| общие типы меню «Пуск» | `model` (новый нижний слой) |

`cargo fmt --all` и `cargo clippy` чисты во всех крейтах; задачи release,
build, fmt, clippy и test зелёные на коммите с тегом.
