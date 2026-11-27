# Trinkets

Extra equipment slots for trinkets, popping out of your inventory screen's armour and offhand slots, and in a menu on G.

Extra equipment slots for the trinkets other mods add: face, hat, necklace, back, cape, charm, glove, two rings, belt, wearable lantern, key, quiver, shoes and a second charm.

What it does:

- On your inventory screen, hover an armour slot or the offhand slot: that group's trinket slots pop out in one framed row, the vanilla slot highlighted in it and the group's slots on either side (the first to its right, the next to its left, and so on: `4 2 0 1 3`). It folds away when the pointer leaves. Groups with no vanilla slot (hand, charm) have their first slot always shown above the offhand; hover it for the rest of the group. An empty slot shows its slot-type picture, a worn item its own tooltip, an empty slot its name. Click a worn item to take it off; click with an item on the cursor to wear it in that slot (the item is parked in a free inventory slot first, and one of it is worn).
- Press G (Open Trinkets in Controls) to open the Trinkets menu: a slot for each trinket place, each showing its picture while empty. Click an item in your inventory to wear it in the first empty slot that takes it (or swap with the first that does); click a worn item to take it off.
- Right-click a trinket in your hand to wear it in the first empty slot that takes it. Backpacks and wireless terminals keep their own use; an elytra still goes in the chest slot.
- An item's tooltip lists the trinket slots it fits, and the key that opens the menu.
- A Totem of Undying worn in a trinket slot saves you from a killing blow, and is used up.
- What you wear is saved with you, kept through death, and shown to your client. When you die without keepInventory your worn trinkets drop where you died (items with Curse of Vanishing are destroyed).
- Other mods read the worn stacks from the `trinkets:worn` player attachment: Sophisticated Backpacks' back slot, Elytra Slot, Things, Radiant Gear and others.

Differences from the original:

- The inventory popup's slots are drawn and clicked by the mod; they are not real slots of the inventory menu. The pinned API (ba1f24e) cannot add slots to the player's inventory menu (`set_screen_slot` only moves slots of an open mod menu, and the player's menu takes no Parts). So shift-click, drag, number-key swaps and recipe viewers do not see them, and a click goes to the server as a payload, not as a menu click. The final form (backed slots in the inventory menu, server-side transfer) waits for sunrise-org/sunrise#4980; the layout (`src/popup.rs`), the sprites and the frame are kept apart from the click path so that move is small.
- Wearing from the cursor parks the cursor stack in a free inventory slot first (the server cannot set the cursor), so with a full inventory the click is refused and nothing changes, where the original swaps with the cursor.
- The popup keeps itself on the screen: when its row would leave the screen on one side it goes the other way if it fits there, else it is pushed back in. The panel moving for the recipe book is followed. This port has no slot type with several copies, so the original's vertical expansion (a column of copies of one slot type) is not there; it would be added with the first such type.
- The popup frame, the empty-slot pictures (and the slot and group names) come from the original's `more_slots.png` and `gui/slots/*.png` (MIT); the lantern, key and quiver slots, which the original has no picture for, keep this port's own.
- The G menu is a two-row chest menu with the slot pictures as placeholder items (the original opens its own screen).
- Right-click wearing is done by the server only; the client does not predict it.
- Dropped trinkets are not put in a grave by a grave mod (universalgraves in the earlier pack): no grave mod is ported. Death drop uses `living_death` and the `minecraft:keep_inventory` game rule.
- The Open Trinkets key listens to keyboard bindings only (a mouse button binding does not open the menu).
- The vanishing-curse check reads `minecraft:enchantments` in the stack's components.

Wearing is first-fit in the slot table's order (the G menu and right click); the popup shows a group's slots in the wiki's order (cape next to the chest slot), which is a separate table (`DISPLAY` in `src/data.rs`).

## Package API for other mods

- The worn stacks are the player attachment `trinkets:worn` (a compound of slot id to item NBT, for example `chest/back` or `chest/cape`; synced to the client and kept through death). A mod reads it with `registrar.attachment_key::<Compound>("trinkets:worn")` in `Hooks::register`, after declaring `trinkets` as a dependency (`AttachmentError::Undeclared` otherwise). Sophisticated Backpacks' back slot (`sophisticatedcore`) does this.
- The slot ids are `head/face`, `head/hat`, `chest/necklace`, `chest/back`, `chest/cape`, `chest/charm`, `hand/glove`, `hand/ring`, `offhand/ring`, `legs/belt`, `legs/lantern`, `legs/key`, `legs/quiver`, `feet/shoes` and `charm/charm`.

## Building

With the Sunrise mod tool (`cargo sunrise-mod`, from the Sunrise SDK) and Rust:

```sh
cargo sunrise-mod build    # compiles against the pinned game, packs target/sunrise-mod/trinkets.smod
cargo sunrise-mod run      # builds it and starts the game with it
cargo test
```

`sunrise-mod.toml` pins the Sunrise build it is made for (`sunrise`, a game commit). Sunrise builds native mods from source on the player's computer, against the exact game build.

## Licence and credits

Trinkets is by Emi (Emily Rose Ploszaj) and C4 (https://github.com/emilyploszaj/trinkets), under the MIT licence; `LICENSE` holds the text and `NOTICE` says where the port comes from. No Java code is included.

Ported to Minecraft 26.3 by lavoide ([lavoide/horizon-ports](https://github.com/lavoide/horizon-ports)), and to Sunrise by arismoko.
