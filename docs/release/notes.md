> **Language note: the English changelog comes first.** It ends at the `---`
> separator, after which the same changelog is repeated in Russian.
> From 0.9.3 on, every WinRust release is published in both languages:
> English first, then Russian.

Windows 10/11 system components rewritten in Rust. Release 0.9.3 answers a
third review: eight reports against 0.9.2, fixed at their causes.

## 1. Start, taskbar and the tray

* **The volume slider works, in both directions.**
  `IID_IAudioEndpointVolume` was `5CDF2B5A-4B4C-4E9C-A714-79CE8D5C9E3E`,
  which is not an interface at all. Every `IMMDevice::Activate` failed with
  `E_NOINTERFACE`, so the getter and the setter both returned `None`: the tray
  could only show its fallback, and dragging the slider snapped straight back.
  The real IID is `5CDF2C82-841E-4546-9722-0CF74078229A`.
* **Every popup hangs from the taskbar's top edge now.**
  `popup_rect_above` / `anchored_above` in `winrust-ui` measure a panel and
  place its *bottom* edge on the bar and its right edge on its button, clamped
  to the screen - so the tray flyouts, the hidden-icons panel, the power list
  and the account list all line up. Every earlier version guessed a height per
  panel and positioned by the top-left corner, which is what dropped them off
  the bottom of the screen.
* **The power and account lists are no longer egui submenus**, which follow the
  cursor and ran off-screen; they are anchored areas like the flyouts.
* **The taskbar icons are centred.** They were drawn inside a nested
  `ui.horizontal`, which starts a Ui only as tall as its tallest child and
  aligns it to the parent's top - so the whole row sat low and the active
  highlight was hard to see. They are laid out in the bar itself and share its
  centre line with the Start button.

## 2. Desktop

* **A long caption fits its icon.** `i18n::wrap_capped` breaks over-long words,
  caps the line count and ellipsises what it drops, and the caption is clipped
  to the icon's cell. A long name used to sprawl out of the selection square.
* **"Personalize" opens Windows Settings** (`ms-settings:personalization`)
  instead of the display control panel.
* **"New" is the machine's own list.** It reads the shell's `ShellNew`
  registrations from both hives and draws each entry with the icon of the
  program that owns the extension, instead of eight hard-coded extensions that
  were mostly absent from the machine.
* **"Align icons to grid" is a real toggle**, in "Sort by" next to the sort
  order. Switched off, an icon stays exactly where it is dropped (its place is
  saved as fractional cell coordinates); switched on, the desktop is tidied.
  The setting was saved before but never consulted, so icons always snapped.

## 3. Start menu

* **Pinned entries are editable.** They live in the session and are saved to
  `%APPDATA%\WinRust\start-pins.json`. Right-clicking a pin gives Open, File
  location, Unpin from Start and Properties - in that order; right-clicking any
  app in the list offers Pin to Start.

## 4. Taskbar previews

* **The preview stays up while you look at it.** The miniature strip is drawn
  outside the button loop, so moving onto a card left the button's hover and
  cleared the preview. A card now counts as still being on the group, and the
  full-screen peek follows the card you are on.

## 5. Explorer

* **Rows are 26 px instead of 22**, so there is a real gap between two names to
  press in and start a rubber-band selection. The empty strip is a rubber-band
  surface in the details view too.
* **The drop target lights up.** The test is geometric against the row's band,
  not `Response::hovered()` on the name cell, so dragging over any column of a
  folder's row highlights it - and it works on the frame the drag begins, when
  `drag_active` has not been set yet.

## Where the new state lives

| new in 0.9.3 | owner |
| --- | --- |
| how a popup is placed above the bar | `winrust_ui::popup_rect_above`, `anchored_above` |
| whether a Start entry is pinned | `session::Session::pins`, `%APPDATA%\WinRust\start-pins.json` |
| where an icon sits when the grid is off | `DesktopIcon::free`, saved as `SavedPos.free` |
| a caption that has to fit a cell | `i18n::wrap_capped` |

`cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
build, fmt, clippy and test jobs are green on the tagged commit.

---

> **Примечание о языке: ниже та же самая версия списка изменений на русском.**
> Начиная с 0.9.3 заметки к каждому релизу WinRust публикуются на двух языках:
> сначала английский, затем русский. Английская часть заканчивается
> разделителем `---` выше.

Компоненты Windows 10/11, переписанные на Rust. Релиз 0.9.3 отвечает на третье
ревью: восемь замечаний по 0.9.2, исправленных по причинам, а не по симптомам.

## 1. Пуск, панель задач и трей

* **Ползунок громкости работает в обе стороны.**
  `IID_IAudioEndpointVolume` был `5CDF2B5A-4B4C-4E9C-A714-79CE8D5C9E3E` —
  это вообще не интерфейс. Каждый вызов `IMMDevice::Activate` падал с
  `E_NOINTERFACE`, поэтому и чтение, и запись возвращали `None`: трей мог
  показать только заглушку, а ползунок при перетаскивании отскакивал обратно.
  Настоящий IID — `5CDF2C82-841E-4546-9722-0CF74078229A`.
* **Все всплывающие панели теперь висят на верхней кромке панели задач.**
  `popup_rect_above` / `anchored_above` в `winrust-ui` измеряют панель и ставят
  её *нижнюю* кромку на панель задач, а правую — по кнопке, с прижатием к
  экрану. Поэтому флайауты трея, панель скрытых значков, меню завершения работы
  и меню пользователя встают ровно. Все прежние версии угадывали высоту
  каждой панели и позиционировали по левому верхнему углу — именно поэтому
  они уезжали за нижний край экрана.
* **Меню завершения работы и меню пользователя больше не подменю egui**:
  подменю идёт за курсором и уходило за экран. Теперь это такие же
  привязанные панели, как остальные флайауты.
* **Значки панели задач выровнены по центру.** Они рисовались внутри
  вложенного `ui.horizontal`, который создаёт Ui высотой по самому высокому
  ребёнку и выравнивает по верху родителя — вся строка сидела низко, и
  подсветка активного окна терялась. Теперь раскладка идёт прямо в панели
  задач и делит её центральную линию с кнопкой «Пуск».

## 2. Рабочий стол

* **Длинная подпись помещается в свой значок.** `i18n::wrap_capped` ломает
  слишком длинные слова, ограничивает число строк и ставит многоточие на
  отброшенном, а подпись обрезается по ячейке значка. Раньше длинное имя
  расползалось за квадрат выделения.
* **«Персонализация» открывает Параметры Windows**
  (`ms-settings:personalization`), а не панель управления экраном.
* **«Создать» — это собственный список машины.** Он читает регистрации
  оболочки `ShellNew` из обоих кустов реестра и рисует каждый пункт значком
  той программы, которой принадлежит расширение, вместо восьми
  захардкоженных расширений, которых на машине чаще всего нет.
* **«Выровнять значки по сетке» — настоящий переключатель**, в «Сортировке»
  рядом с порядком сортировки. Если выключить, значок остаётся ровно там, где
  его отпустили (место сохраняется в дробных координатах ячейки); если
  включить — рабочий стол приводится в порядок. Раньше настройка
  сохранялась, но нигде не проверялась, поэтому значки всегда прилипали.

## 3. Меню «Пуск»

* **Закреплённые записи можно править.** Они живут в сессии и сохраняются в
  `%APPDATA%\WinRust\start-pins.json`. Правый клик по закреплённой записи
  даёт «Открыть», «Расположение файла», «Удалить закрепление» и «Свойства»
  именно в этом порядке; правый клик по любому приложению в списке
  предлагает «Закрепить в меню Пуск».

## 4. Предпросмотр на панели задач

* **Предпросмотр остаётся на экране, пока вы на него смотрите.** Полоса
  миниатюр рисуется вне цикла кнопок, поэтому при переходе на карточку
  пропадал hover кнопки и вместе с ним предпросмотр. Теперь карточка
  считается частью группы, и полноэкранный просмотр следует за той
  карточкой, на которой находится указатель.

## 5. Проводник

* **Строки по 26 px вместо 22**, поэтому между двумя именами появился
  реальный зазор, в который можно нажать и начать выделение рамкой. Пустая
  полоса в режиме «Таблица» тоже стала поверхностью для рамки.
* **Папка-цель подсвечивается.** Проверка геометрическая, по полосе строки, а
  не `Response::hovered()` на ячейке имени, поэтому подсветка срабатывает при
  перетаскивании над любым столбцом строки папки — и уже в кадре начала
  перетаскивания, когда `drag_active` ещё не выставлен.

## Где живёт новое состояние

| новое в 0.9.3 | владелец |
| --- | --- |
| как панель ставится над панелью задач | `winrust_ui::popup_rect_above`, `anchored_above` |
| закреплён ли пункт меню «Пуск» | `session::Session::pins`, `%APPDATA%\WinRust\start-pins.json` |
| где стоит значок при выключенной сетке | `DesktopIcon::free`, сохраняется как `SavedPos.free` |
| подпись, которая обязана влезть в ячейку | `i18n::wrap_capped` |

`cargo fmt --all` и `cargo clippy` чисты во всех крейтах; задачи release,
build, fmt, clippy и test зелёные на коммите с тегом.
