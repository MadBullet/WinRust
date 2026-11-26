> **Language note: the English changelog comes first.** It ends at the `---`
> separator, after which the same changelog is repeated in Russian.
> From 0.9.3 on, every WinRust release is published in both languages:
> English first, then Russian.

Windows 10/11 system components rewritten in Rust. Release 0.9.5 answers a
fifth review: six reports against 0.9.4, fixed at their causes.

## 1. Start menu

* **The account and power lists have a surface and an outline.** Both were
  bare `Area`s: the buttons painted straight onto whatever was behind the
  menu, so the list read as a loose column of labels with no panel around
  it. They now use the same `Frame::popup` the tray's flyouts use - same
  fill, same 1 px line, same 10 px rounding.
* **The pinned grid is free-form.** Empty slots used to be nothing at all,
  so a tile could only be dropped on a tile and the grid could never be
  rearranged the way Windows 11 allows. A spare row of dimmed placeholder
  slots is drawn, every slot is a drop target, and the destination is
  clamped to "one past the last tile" instead of being rejected.
* **A drag has a ghost.** The tile being dragged is drawn at 40 % opacity in
  place, and the slot under the pointer is outlined in blue, so it is
  visible both where the tile came from and where it will land.

## 2. Taskbar

* **Only real program windows get a button.** The window list filtered on
  "visible with a title", which still includes helper windows programs
  create for their own use. Owned windows, tool windows and windows that
  refuse activation are excluded now, the way the shell excludes them.
* **The active button is chosen by window, not by process.** Grouping is by
  program, and a program with several windows only has one of them in front;
  comparing process ids marked every window of a multi-process program as
  active at once. The foreground handle is sampled every frame and matched
  against the group's handles.
* **Icons come from the program's own path.** Three sources, cheapest first:
  the window's icon, the executable's icon, and - when the window list could
  not read the image path - the process's own path asked again at drawing
  time, then the shell's own shortcut for that program name.
* **Clicking a grouped button raises the window you actually used.** It
  raised whichever window the process owned, which is not necessarily the
  one that was in front.
* **The preview no longer disappears on the way to it.** The group being
  previewed is remembered for a short grace period instead of being derived
  from "is the pointer on a button", so crossing the six-pixel gap between
  the button and the strip does not dismiss it. The strip is rebuilt
  whenever the pointer is still around, so the hit test can run on it.

## 3. Explorer

* **Favourites can be reordered by dragging.** The dragged row was a local
  variable, `drag_started` is true for exactly one frame, and the release
  arrives several frames later - the two could never meet. The row index is
  now remembered in egui's memory for the duration of the gesture, and the
  list is reordered on release, with the insertion line drawn where the row
  will land.

## 4. The hidden-icons flyout

* **Every row has hover feedback.** The rows were frameless buttons, and an
  egui button without a frame has no visual state at all, so the pointer
  gave nothing back before the click. A row now paints a soft fill and an
  accent outline while it is under the pointer.

## Where the new state lives

| new in 0.9.5 | owner |
| --- | --- |
| the program being previewed | `DesktopApp::hovered_group` |
| how long the preview may survive a gap | `DesktopApp::preview_at` |
| the window in front, for the active button | `DesktopApp::active_hwnd` |
| the favourite being dragged | egui memory, `Id::new("explorer_fav_drag")` |
| the surface of the two footer lists | `start::popup_surface`, `popup_line` |

`cargo fmt --all` and `cargo clippy` are clean on every crate; the release,
build, fmt, clippy and test jobs are green on the tagged commit.

---

> **Примечание о языке: ниже та же самая версия списка изменений на русском.**
> Начиная с 0.9.3 заметки к каждому релизу WinRust публикуются на двух языках:
> сначала английский, затем русский. Английская часть заканчивается
> разделителем `---` выше.

Компоненты Windows 10/11, переписанные на Rust. Релиз 0.9.5 отвечает на
пятое ревью: шесть замечаний по 0.9.4, исправленных по причинам.

## 1. Меню «Пуск»

* **У списков пользователя и завершения работы есть фон и обводка.** Оба были
  голыми `Area`: кнопки рисовались прямо поверх того, что было за меню,
  поэтому список читался как рыхлая колонка подписей без панели вокруг. Теперь
  они используют тот же `Frame::popup`, что и флайауты трея — та же заливка,
  та же линия в 1 px, то же скругление 10 px.
* **Сетка закреплённых стала свободной.** Пустых ячеек раньше не было вовсе,
  поэтому плитку можно было бросить только на плитку, и переставить сетку так,
  как это позволяет Windows 11, было нельзя. Теперь рисуется запасной ряд
  приглушённых пустых ячеек, каждая ячейка принимает сброс, а место вставки
  прижимается к «сразу за последней плиткой» вместо отказа.
* **У перетаскивания есть призрак.** Перетаскиваемая плитка рисуется на месте
  с прозрачностью 40 %, а ячейка под курсором обводится синим — видно и
  откуда плитка, и куда она попадёт.

## 2. Панель задач

* **Кнопку получают только настоящие окна программ.** Список окон фильтровался
  по «видимо и с заголовком», а под это попадают и служебные окна, которые
  программы создают для себя. Теперь окна с владельцем, инструментальные окна
  и окна, отказывающиеся от активации, исключены — так же, как это делает
  оболочка Windows.
* **Активная кнопка выбирается по окну, а не по процессу.** Группировка идёт по
  программе, а у программы с несколькими окнами впереди только одно; сравнение
  идентификаторов процессов помечало активными все окна такой программы сразу.
  Хэндл переднего окна снимается каждый кадр и сверяется с хэндлами группы.
* **Значки берутся из пути самой программы.** Три источника, от дешёвого:
  значок окна, значок исполняемого файла и — когда список окон не смог прочитать
  путь образа — повторный запрос пути у процесса в момент отрисовки, а затем
  ярлык этой программы из поставки оболочки.
* **Нажатие на сгруппированную кнопку поднимает то окно, которым вы
  пользовались.** Раньше поднималось то окно процесса, которое попалось, — не
  обязательно то, что было впереди.
* **Превью больше не пропадает по дороге к нему.** Показываемая группа
  запоминается на короткий льготный период вместо того, чтобы выводиться из
  «курсор на кнопке», поэтому переход через шестипиксельный зазор между кнопкой
  и полосой не закрывает превью. Полоса перестраивается, пока курсор рядом, —
  значит, проверка попадания по ней успевает сработать.

## 3. Проводник

* **«Избранное» переставляется перетаскиванием.** Перетаскиваемая строка была
  локальной переменной, `drag_started` истинно ровно один кадр, а отпускание
  приходит несколькими кадрами позже — эти два события не могли встретиться.
  Теперь номер строки на время жеста хранится в памяти egui, а список
  переставляется при отпускании; линия вставки рисуется там, куда строка
  попадёт.

## 4. Панель скрытых значков

* **У каждой строки есть отклик на наведение.** Строки были кнопками без рамки,
  а кнопка egui без рамки не имеет визуальных состояний вовсе — до щелчка
  курсор ничего не сообщал. Теперь строка под курсором получает мягкую заливку
  и обводку цветом акцента.

## Где живёт новое состояние

| новое в 0.9.5 | владелец |
| --- | --- |
| программа, чьё превью показывается | `DesktopApp::hovered_group` |
| сколько превью может пережить разрыв | `DesktopApp::preview_at` |
| окно впереди, для активной кнопки | `DesktopApp::active_hwnd` |
| перетаскиваемая строка «Избранного» | память egui, `Id::new("explorer_fav_drag")` |
| фон двух нижних списков «Пуска» | `start::popup_surface`, `popup_line` |

`cargo fmt --all` и `cargo clippy` чисты во всех крейтах; задачи release,
build, fmt, clippy и test зелёные на коммите с тегом.
