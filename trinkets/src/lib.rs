#![deny(unsafe_code)]
//! Trinkets 3.7.2 (Emi, C4; MIT): the player's extra equipment slots, written on Sunrise's typed mod API
//! (no `[[native.mixin]]`, no game module).
//!
//! - The worn stacks are a player attachment, `trinkets:worn`: a compound of slot id -> item NBT
//!   (`{id, count, components}`), saved with the player, kept through death, and synced to the player's client.
//!   Other packages (sophisticatedcore's back slot, elytraslot, things, radiantgear) read it with
//!   `attachment_key::<Compound>("trinkets:worn")` after declaring this package as a dependency.
//! - The menu (TrinketInventoryScreen's slots) is a vanilla chest menu the mod fills (`open_chest_menu`),
//!   opened by the Trinkets key (G, `key.trinkets.open` in Controls): the client sends `trinkets:open` and the
//!   server opens it on its next tick. A click on an inventory item wears it, a click on a worn one takes it off.
//! - The player's inventory screen shows the groups too (head, chest, legs, feet, offhand, hand, charm): hovering an
//!   armour or offhand slot fans its group out of it in a framed row (kept on the screen, flipped or pushed back),
//!   hand and charm have a small anchor left of the panel; an empty slot shows its slot-type icon. The pinned API
//!   cannot add slots to the player inventory menu (`set_screen_slot` only moves slots of an open mod menu), so
//!   `screen_render_post` draws them and `screen_mouse_button_pressed_pre` takes the clicks, which go to the
//!   server as `trinkets:slot` (a click with a stack on the cursor first parks that stack in a free inventory slot
//!   with `MenuRef::click`). `popup` holds the layout, apart from the click path: backed slots (sunrise#4980) replace
//!   the click path only. The release of a press the mod took is the mod's; any other release is the screen's.
//! - A right click with a trinket wears it in the first empty slot that takes it (TrinketItem.use).
//! - The tooltip names the slots an item fits.
//! - A worn Totem of Undying saves the player: `living_death_protection` hands the engine the worn totem as the
//!   protector, takes it out of its slot and syncs `trinkets:worn`.
//! - Death: unless keepInventory, the worn stacks drop where the player died (vanishing-cursed ones vanish).
pub mod data;
pub mod popup;
pub use data::SLOTS;
use popup::{CELL, FRAME_CAP, FRAME_H, GROUPS, anchor_cell, members, open_group, over, place};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use mod_api::{
    AttachmentCodec, AttachmentDescriptor, AttachmentKey, ClientHost, ClientTickPostEvent,
    Compound, Flow, Hand, HitHost, Holder, Hooks, Host, HostSide, IPayloadContext, IPayloadHandler,
    Identifier, InputKeyEvent, InteractionResult, ItemResource, ItemStack, ItemTooltipEvent,
    KeyAction, KeyRegistration, LivingDeathEvent, LivingDeathProtectionEvent, MenuClick,
    MenuClickedEvent, MenuPart, MenuRef, MenuRemovedEvent, MenuReplace, MouseButton, PayloadBytes,
    PayloadHandleError, PayloadId, PlayerLoggedInEvent, PlayerRef, PlayerRespawnedEvent, Registrar,
    RegistrationContext, RightClickItemEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenRenderPostEvent, ScreenView, ServerHost,
    ServerTickEvent, ServerTickPhase, Tag, Text, TooltipElement,
};

/// The id of the worn-stacks attachment other packages look up.
pub const WORN_ID: &str = "trinkets:worn";
const OPEN: &str = "trinkets:open";
/// A click on a trinket slot of the inventory screen: [kind, trinket slot, inventory slot].
const SLOT: &str = "trinkets:slot";
const TAKE: u8 = 0;
const WEAR: u8 = 1;
const KEY: &str = "open";
const MENU: &str = "minecraft:generic_9x2";

static WORN: RwLock<Option<AttachmentKey<Compound>>> = RwLock::new(None);

fn worn_key() -> Option<AttachmentKey<Compound>> {
    WORN.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether slot `i` takes `item` (its trinkets tag).
pub fn accepts(i: usize, item: &str) -> bool {
    SLOTS.get(i).is_some_and(|s| s.3.contains(&item))
}

/// Whether some slot takes `item`.
pub fn is_trinket(item: &str) -> bool {
    SLOTS.iter().any(|s| s.3.contains(&item))
}

/// Items tagged into a trinket slot that aren't TrinketItems in Java: their own use comes first (a backpack
/// opens, a wireless terminal opens its terminal), so a right click doesn't wear them; the menu still does.
fn own_use(id: &str) -> bool {
    id.starts_with("sophisticatedbackpacks:") || id == "toms_storage:ts.adv_wireless_terminal"
}

/// The custom_data key of a stand-in for a worn item the game can't make (a mod that isn't installed): the saved
/// entry, written back unchanged.
const UNKNOWN: &str = "trinkets:unknown";

fn custom_data(stack: &ItemStack) -> Compound {
    match stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()))
    {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    }
}

/// The saved entry a stand-in carries.
fn unknown_entry(stack: &ItemStack) -> Option<Compound> {
    match custom_data(stack).get(&UNKNOWN.into())? {
        Tag::Compound(c) => Some(c.clone()),
        _ => None,
    }
}

fn is_unknown(stack: &ItemStack) -> bool {
    unknown_entry(stack).is_some()
}

fn text_line(t: &str, color: Option<&str>) -> Tag {
    let mut line = vec![("text", Tag::String(t.into())), ("italic", Tag::Byte(0))];
    if let Some(color) = color {
        line.push(("color", Tag::String(color.into())));
    }
    Tag::compound(line)
}

/// A barrier standing in for the saved entry `raw` ("Not ported: vc_gliders:paraglider_wood"); it stays in its
/// slot and is saved back as it was.
fn stand_in(host: &dyn Host, raw: &Compound) -> Option<ItemStack> {
    let id = match raw.get(&"id".into())? {
        Tag::String(s) => s.to_string_lossy(),
        _ => return None,
    };
    if id.is_empty() || id == "minecraft:air" {
        return None;
    }
    let key = host.item_key(&Identifier::parse("minecraft:barrier").ok()?)?;
    let mut st = ItemStack::new(key, 1)?;
    let mut patch = Compound::new();
    patch.insert(
        "minecraft:custom_name".into(),
        text_line(&format!("Not ported: {id}"), Some("red")),
    );
    patch.insert(
        "minecraft:lore".into(),
        Tag::List(vec![text_line(
            "Kept in this slot until its mod is installed",
            Some("gray"),
        )]),
    );
    patch.insert(
        "minecraft:custom_data".into(),
        Tag::compound([(UNKNOWN, Tag::Compound(raw.clone()))]),
    );
    st.apply_components(&patch).ok()?;
    Some(st)
}

/// The player's worn stacks, one per slot of `SLOTS`. A worn item the game can't make comes back as a stand-in
/// (`stand_in`), so the slot stays taken and `set_worn` writes the entry back.
fn worn(host: &mut dyn Host, uuid: u128) -> Vec<Option<ItemStack>> {
    let c = worn_key()
        .and_then(|key| {
            key.get_existing(host, Holder::Player(uuid), |c| c.cloned())
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    let reader: &dyn Host = &*host;
    SLOTS
        .iter()
        .map(|s| match c.get(&s.0.into()) {
            Some(Tag::Compound(t)) => {
                ItemStack::from_nbt(reader, t).or_else(|| stand_in(reader, t))
            }
            _ => None,
        })
        .collect()
}

/// Writes the worn stacks. Entries under ids that aren't slots of this table are kept as they are.
fn set_worn(host: &mut dyn ServerHost, uuid: u128, stacks: &[Option<ItemStack>]) -> bool {
    let Some(key) = worn_key() else {
        return false;
    };
    let mut c = key
        .get_existing(host, Holder::Player(uuid), |c| c.cloned())
        .ok()
        .flatten()
        .unwrap_or_default();
    for s in SLOTS {
        c.remove(&s.0.into());
    }
    for (s, stack) in SLOTS.iter().zip(stacks) {
        let Some(stack) = stack.as_ref().filter(|s| s.count() > 0) else {
            continue;
        };
        if let Some(t) = unknown_entry(stack).or_else(|| stack.to_nbt(&*host)) {
            c.insert(s.0.into(), Tag::Compound(t));
        }
    }
    let ok = key.set(host, Holder::Player(uuid), c).is_ok();
    let _ = key.sync_data(host, Holder::Player(uuid));
    ok
}

/// The GUI sprite of slot `i`'s slot-type icon (`trinkets:gui/slots/<slot>` of the original).
fn icon(i: usize) -> String {
    let name = SLOTS[i].0.rsplit('/').next().unwrap_or("empty");
    format!("trinkets:slots/{name}")
}

fn item_id(host: &dyn Host, stack: &ItemStack) -> String {
    stack.item().id(host).unwrap_or_default().to_owned()
}

/// Whether the components' `minecraft:enchantments` (id -> level) hold the curse of vanishing.
fn vanishes(components: &Compound) -> bool {
    matches!(
        components.get(&"minecraft:enchantments".into()),
        Some(Tag::Compound(e)) if e.contains_key(&"minecraft:vanishing_curse".into())
    )
}

fn has_vanishing_curse(stack: &ItemStack) -> bool {
    stack.components().is_some_and(|c| vanishes(c.as_nbt()))
}

/// `stack` with `count` (a count of 0 is nothing).
fn with_count(stack: &ItemStack, count: u32) -> Option<ItemStack> {
    if count == 0 {
        return None;
    }
    let mut s = stack.clone();
    s.set_count(count).ok()?;
    Some(s)
}

// ---- the menu ----

/// The chest rows the slots fill.
pub fn rows() -> u8 {
    SLOTS.len().div_ceil(9).clamp(1, 6) as u8
}

/// What the menu shows: each worn stack, or the slot's placeholder named for the slot.
fn menu_items(host: &dyn Host, worn: &[Option<ItemStack>]) -> Vec<Option<ItemStack>> {
    let mut out: Vec<Option<ItemStack>> = SLOTS
        .iter()
        .zip(worn)
        .map(|(s, w)| {
            w.clone().or_else(|| {
                let key = host.item_key(&Identifier::parse(s.2).ok()?)?;
                let mut p = ItemStack::new(key, 1)?;
                let mut patch = Compound::new();
                patch.insert(
                    "minecraft:custom_name".into(),
                    text_line(&format!("{} (empty)", s.1), None),
                );
                patch.insert(
                    "minecraft:lore".into(),
                    Tag::List(vec![text_line(
                        "Click a trinket in your inventory to wear it.",
                        Some("gray"),
                    )]),
                );
                let _ = p.apply_components(&patch);
                Some(p)
            })
        })
        .collect();
    out.resize(rows() as usize * 9, None);
    out
}

/// A menu slot past the menu's own: the player's inventory slot (main 9..36 first, then the hotbar).
pub fn inventory_slot(size: usize, slot: i32) -> Option<u32> {
    let rel = usize::try_from(slot).ok()?.checked_sub(size)?;
    match rel {
        0..27 => Some(rel as u32 + 9),
        27..36 => Some(rel as u32 - 27),
        _ => None,
    }
}

/// The main inventory (slots 0..36), and whether every slot was read as it is. A slot whose stack can't be handed
/// to a mod reads as empty: it is not, so nothing is written to a slot that only looks empty unless the read was
/// whole (`give_stack` places the rest instead).
fn inventory(host: &dyn Host, player: PlayerRef) -> (Vec<Option<ItemStack>>, bool) {
    player
        .with_inventory(host, |inventory| {
            let mut inv = vec![None; 36];
            let mut whole = true;
            for (slot, view) in inventory.iter() {
                if (slot as usize) < 36 {
                    match view.project() {
                        Some(stack) => inv[slot as usize] = Some(stack),
                        None => whole = false,
                    }
                }
            }
            (inv, whole)
        })
        .unwrap_or_else(|| (vec![None; 36], false))
}

/// Drops `stack` where the player stands.
fn drop_at(host: &mut dyn ServerHost, player: PlayerRef, stack: ItemStack) {
    let Some(dim) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(level) = host.level(&dim) else {
        return;
    };
    let Some(pos) = player.entity().position(&*host) else {
        return;
    };
    let _ = level.pop_resource(
        host,
        [
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        ],
        stack,
    );
}

/// Puts `stack` in the player's inventory as the game's `Inventory.add` does; what doesn't fit is thrown to the
/// player. Whether it was given.
fn give(host: &mut dyn ServerHost, player: PlayerRef, stack: ItemStack) -> bool {
    match player.give_stack(host, stack) {
        Ok(rest) => {
            if let Some(rest) = rest.filter(|r| r.count() > 0) {
                drop_at(host, player, rest);
            }
            true
        }
        Err(e) => {
            eprintln!("[trinkets] give_stack: {e:?}");
            false
        }
    }
}

/// What a click does to the worn stacks and the inventory, before any of it is written.
#[derive(Debug, PartialEq)]
enum Plan {
    /// Nothing moves; the player is told why.
    Refuse(&'static str),
    /// Inventory slot `inv` is worn in slot `i`: one item goes, and what was there comes back.
    Wear { i: usize, inv: usize },
}

/// Which trinket slots can take an item, as (index, taken by an unknown stand-in).
fn plan_wear(fits: &[usize], occupied: &[bool], count: u32, room: bool, inv: usize) -> Plan {
    match fits
        .iter()
        .copied()
        .find(|&i| !occupied[i])
        .or(fits.first().copied())
    {
        Some(i) if count == 1 || !occupied[i] || room => Plan::Wear { i, inv },
        Some(_) => Plan::Refuse("No room in your inventory"),
        None => Plan::Refuse("That doesn't go in a trinket slot"),
    }
}

/// The writes of a click.
trait Store<I> {
    fn set_slot(&mut self, slot: u32, stack: Option<I>) -> bool;
    fn set_worn(&mut self, worn: &[Option<I>]) -> bool;
    fn give(&mut self, stack: I) -> bool;
}

/// Does a click's writes all or none: `slot` is (inventory slot, new stack, old stack), then the worn stacks
/// (`old` to `new`), then `give` goes to the inventory. When a step fails the ones before it are undone and it
/// is false.
fn commit<I: Clone>(
    store: &mut impl Store<I>,
    slot: Option<(u32, Option<I>, Option<I>)>,
    old: &[Option<I>],
    new: &[Option<I>],
    give: Option<I>,
) -> bool {
    if let Some((s, to, _)) = &slot
        && !store.set_slot(*s, to.clone())
    {
        return false;
    }
    let mut ok = store.set_worn(new);
    if ok && let Some(g) = give {
        ok = store.give(g);
        if !ok {
            store.set_worn(old);
        }
    }
    if !ok && let Some((s, _, from)) = slot {
        store.set_slot(s, from);
    }
    ok
}

/// The server's stacks as `commit` writes them.
struct Real<'a> {
    host: &'a mut dyn ServerHost,
    player: PlayerRef,
    uuid: u128,
}

impl Store<ItemStack> for Real<'_> {
    fn set_slot(&mut self, slot: u32, stack: Option<ItemStack>) -> bool {
        match self.player.set_inventory_slot(&mut *self.host, slot, stack) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("[trinkets] set_inventory_slot {slot}: {e:?}");
                false
            }
        }
    }

    fn set_worn(&mut self, worn: &[Option<ItemStack>]) -> bool {
        set_worn(&mut *self.host, self.uuid, worn)
    }

    fn give(&mut self, stack: ItemStack) -> bool {
        give(&mut *self.host, self.player, stack)
    }
}

#[derive(Default)]
struct Shared {
    /// The worn Totem of Undying of each player, by entity id: what `living_death_protection` answers with. The hook's
    /// host reads no item registry, so the stack is kept here whenever the worn stacks change or the player appears.
    totems: Mutex<HashMap<i32, WornTotem>>,
    /// Players who pressed the key, to open the menu for on the next server tick.
    opens: Mutex<Vec<u128>>,
    /// The open trinket menu's window of each player.
    menus: Mutex<HashMap<u128, i32>>,
    /// Slot clicks of the inventory screen to do on the next server tick.
    slots: Mutex<Vec<(u128, SlotClick)>>,
    /// Client: the local player's worn stacks, read each tick while the inventory screen is open (the screen's
    /// hooks cannot read attachments).
    view: Mutex<Vec<Option<ItemStack>>>,
    /// Client: the group whose popup is open on the inventory screen.
    open: Mutex<Option<usize>>,
    /// Client: trinket slots clicked on the inventory screen, handled on the next client tick.
    clicks: Mutex<Vec<Click>>,
    /// Client: which presses on the inventory screen the mod took, so only their releases are the mod's.
    presses: Mutex<Presses>,
}

/// A click on a trinket slot of the inventory screen, as it goes to the server. `expect` is the item the player
/// saw: the worn one (take off) or the one parked in inventory slot `inv` (wear). The server drops the click when
/// the stack there is another one by then.
#[derive(Debug, PartialEq)]
struct SlotClick {
    kind: u8,
    slot: usize,
    inv: usize,
    expect: String,
}

impl SlotClick {
    fn encode(&self) -> Vec<u8> {
        let mut body = vec![self.kind, self.slot as u8, self.inv as u8];
        body.extend_from_slice(self.expect.as_bytes());
        body
    }

    fn decode(body: &[u8]) -> Option<SlotClick> {
        let [kind @ (TAKE | WEAR), slot, inv, expect @ ..] = body else {
            return None;
        };
        if (*slot as usize) >= SLOTS.len() || *inv >= 36 {
            return None;
        }
        Some(SlotClick {
            kind: *kind,
            slot: *slot as usize,
            inv: *inv as usize,
            expect: String::from_utf8(expect.to_vec()).ok()?,
        })
    }
}

/// A press on a trinket slot, noted when it happens: what was on the cursor then (its item). The click is done on
/// the next tick and only if the screen is still the inventory and the cursor is the same.
struct Click {
    slot: usize,
    carried: Option<String>,
}

/// The mouse buttons (GLFW index, as a bit) whose press the mod took. Its release is the mod's too, wherever the
/// pointer is by then; the release of a press the screen took is the screen's.
#[derive(Default)]
struct Presses(u32);

impl Presses {
    fn bit(button: i32) -> u32 {
        u32::try_from(button)
            .ok()
            .and_then(|b| 1u32.checked_shl(b))
            .unwrap_or(0)
    }

    /// A press of `button`; `taken` when the mod took it. A new press ends the one before it.
    fn press(&mut self, button: i32, taken: bool) {
        self.0 &= !Self::bit(button);
        if taken {
            self.0 |= Self::bit(button);
        }
    }

    /// A release of `button`: whether it ends a press the mod took.
    fn release(&mut self, button: i32) -> bool {
        let taken = self.0 & Self::bit(button) != 0;
        self.0 &= !Self::bit(button);
        taken
    }
}

/// A worn totem: whose it is, the slot it sits in and the stack.
struct WornTotem {
    uuid: u128,
    slot: usize,
    stack: ItemStack,
}

/// The id of the item that protects from death.
const TOTEM: &str = "minecraft:totem_of_undying";

/// The first slot whose item is a totem.
fn first_totem(ids: &[Option<&str>]) -> Option<usize> {
    ids.iter().position(|id| *id == Some(TOTEM))
}

impl Shared {
    /// Re-reads the player's worn stacks and keeps the first worn totem for `living_death_protection`.
    fn refresh_totem(&self, host: &mut dyn ServerHost, uuid: u128) {
        let Some(entity) = host.player(uuid).map(|p| p.entity().id() as i32) else {
            return;
        };
        let stacks = worn(host, uuid);
        let ids: Vec<Option<String>> = stacks
            .iter()
            .map(|s| {
                s.as_ref()
                    .filter(|s| !is_unknown(s))
                    .map(|s| item_id(&*host, s))
            })
            .collect();
        let refs: Vec<Option<&str>> = ids.iter().map(|i| i.as_deref()).collect();
        let mut totems = lock(&self.totems);
        totems.retain(|_, t| t.uuid != uuid);
        if let Some(slot) = first_totem(&refs)
            && let Some(stack) = stacks[slot].clone()
        {
            totems.insert(entity, WornTotem { uuid, slot, stack });
        }
    }
}

/// The player's open menu, when it is the window `window`.
fn live(host: &dyn ServerHost, uuid: u128, window: i32) -> Option<MenuRef> {
    let menu = host.player(uuid)?.container_menu(host)?;
    (menu.container_id(host) == Some(window)).then_some(menu)
}

impl Shared {
    fn open(&self, host: &mut dyn ServerHost, uuid: u128) {
        // Already looking at it: don't rebuild the menu and resend the inventory.
        let window = lock(&self.menus).get(&uuid).copied();
        if window.is_some_and(|w| live(&*host, uuid, w).is_some()) {
            return;
        }
        let stacks = worn(host, uuid);
        let items = menu_items(&*host, &stacks);
        let Some(player) = host.player(uuid) else {
            return;
        };
        match player.open_chest_menu(
            host,
            rows(),
            Text::literal("Trinkets"),
            items,
            MenuReplace::CloseFirst,
        ) {
            Ok(menu) => {
                if let Some(window) = menu.container_id(&*host) {
                    lock(&self.menus).insert(uuid, window);
                }
            }
            Err(e) => eprintln!("[trinkets] open_chest_menu: {e:?}"),
        }
    }

    /// A click on the inventory screen's trinket slot, on the server tick: takes the stack off, or wears the stack in
    /// inventory slot `inv` there. The click is for the player's own inventory window and for the stack the
    /// player saw; with another window open or another stack there it does nothing. The inventory menu is not
    /// the trinket menu, so no window is refreshed.
    fn slot(&self, host: &mut dyn ServerHost, uuid: u128, c: SlotClick) {
        if live(&*host, uuid, 0).is_none() {
            return;
        }
        let size = rows() as usize * 9;
        if c.kind == TAKE {
            self.click(host, uuid, -1, c.slot as i32, None, Some(&c.expect));
        } else {
            let rel = if c.inv >= 9 { c.inv - 9 } else { c.inv + 27 };
            self.click(
                host,
                uuid,
                -1,
                (size + rel) as i32,
                Some(c.slot),
                Some(&c.expect),
            );
        }
    }

    /// A click in a trinket menu: all of it or nothing. `expect` is the item the clicker saw in the slot.
    fn click(
        &self,
        host: &mut dyn ServerHost,
        uuid: u128,
        window: i32,
        slot: i32,
        only: Option<usize>,
        expect: Option<&str>,
    ) {
        let Some(player) = host.player(uuid) else {
            return;
        };
        let size = rows() as usize * 9;
        let before = worn(host, uuid);
        let mut worn = before.clone();
        let (inv, complete) = inventory(&*host, player);
        // Room for one more stack: a free slot, or (with an incomplete read) the game decides and throws what
        // doesn't fit.
        let room = !complete || inv.iter().any(Option::is_none);
        let mut change: Option<(u32, Option<ItemStack>, Option<ItemStack>)> = None;
        let mut give: Option<ItemStack> = None;
        let mut message: Option<&str> = None;
        if let Ok(i) = usize::try_from(slot)
            && i < SLOTS.len()
        {
            // TrinketSlot: take a worn stack off, into the inventory.
            if worn[i].as_ref().is_some_and(is_unknown) {
                let _ = player.action_bar(
                    host,
                    Text::literal("This item's mod isn't installed: it stays in its slot"),
                );
                return;
            }
            let Some(stack) = worn[i].take() else {
                return;
            };
            if expect.is_some_and(|e| e != item_id(&*host, &stack)) {
                return;
            }
            if room {
                give = Some(stack);
            } else {
                worn[i] = Some(stack);
                message = Some("No room in your inventory");
            }
        } else if let Some(s) = inventory_slot(size, slot) {
            // A click on an inventory item: into the first empty slot that takes it, else swapped with the
            // first one that does.
            let s = s as usize;
            let Some(stack) = inv[s].clone() else {
                return;
            };
            let id = item_id(&*host, &stack);
            if expect.is_some_and(|e| e != id) {
                return;
            }
            let fits: Vec<usize> = (0..SLOTS.len())
                .filter(|&i| {
                    only.is_none_or(|o| o == i)
                        && accepts(i, &id)
                        && !worn[i].as_ref().is_some_and(is_unknown)
                })
                .collect();
            let occupied: Vec<bool> = worn.iter().map(Option::is_some).collect();
            match plan_wear(&fits, &occupied, stack.count(), room, s) {
                Plan::Wear { i, .. } => {
                    change = Some((
                        s as u32,
                        with_count(&stack, stack.count() - 1),
                        Some(stack.clone()),
                    ));
                    give = worn[i].replace(with_count(&stack, 1).unwrap_or(stack));
                }
                Plan::Refuse(m) => message = Some(m),
            }
        } else {
            return;
        }
        if message.is_none() {
            let mut store = Real {
                host: &mut *host,
                player,
                uuid,
            };
            if !commit(&mut store, change, &before, &worn, give) {
                message = Some("That didn't work: nothing moved");
            }
        }
        self.refresh_totem(host, uuid);
        let now = self::worn(host, uuid);
        let items = menu_items(&*host, &now);
        if let Some(menu) = live(&*host, uuid, window)
            && let Err(e) = menu.set_items(host, items)
        {
            eprintln!("[trinkets] set_items: {e:?}");
        }
        if let Some(m) = message {
            let _ = player.action_bar(host, Text::literal(m));
        }
    }
}

/// The trinket menu's clicks: the menu is the vanilla two-row chest menu, so the part is asked for every such
/// menu and takes only the window it opened.
struct TrinketMenu(Arc<Shared>);

#[mod_api::sunrise_part]
impl MenuPart for TrinketMenu {
    fn menu_clicked(&self, host: &mut dyn ServerHost, event: &mut MenuClickedEvent<'_>) -> Flow {
        if lock(&self.0.menus).get(&event.uuid) != Some(&event.window) {
            return Flow::Continue;
        }
        // PICKUP and QUICK_MOVE; drags, swaps and throws do nothing, and nothing moves on its own.
        if matches!(event.mode, 0 | 1) {
            self.0
                .click(host, event.uuid, event.window, event.slot, None, None);
        }
        Flow::Cancel
    }

    fn menu_removed(&self, _host: &mut dyn ServerHost, event: &mut MenuRemovedEvent<'_>) {
        let mut menus = lock(&self.0.menus);
        if menus.get(&event.uuid) == Some(&event.window) {
            menus.remove(&event.uuid);
        }
    }
}

struct OpenHandler(Arc<Shared>);

impl IPayloadHandler for OpenHandler {
    fn handle(
        &self,
        _payload: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        // One pending open per player, however often the key packet comes.
        if let Ok(p) = context.player() {
            let uuid = p.uuid();
            let mut opens = lock(&self.0.opens);
            if !opens.contains(&uuid) {
                opens.push(uuid);
            }
        }
        Ok(())
    }
}

struct SlotHandler(Arc<Shared>);

impl IPayloadHandler for SlotHandler {
    fn handle(
        &self,
        payload: PayloadBytes,
        context: &mut dyn IPayloadContext,
    ) -> Result<(), PayloadHandleError> {
        if let (Ok(p), Some(click)) = (context.player(), SlotClick::decode(&payload.0)) {
            lock(&self.0.slots).push((p.uuid(), click));
        }
        Ok(())
    }
}

// ---- the inventory screen ----

/// Whether `screen` is the player's own inventory (not a container, a mount's inventory or creative).
fn player_inventory(screen: &ScreenView<'_>) -> bool {
    screen.id == "inventory"
        && screen.menu.is_none()
        && screen.title.is_none()
        && screen.mod_screen.is_none()
}

/// The GUI size of the window in pixels: the screen the popup has to stay in (no limit when unknown).
fn gui_size(host: &dyn ClientHost) -> [f32; 2] {
    match (host.window_size(), host.gui_scale()) {
        (Some([w, h]), Some(scale)) => {
            let scale = scale.max(1);
            [w.div_ceil(scale) as f32, h.div_ceil(scale) as f32]
        }
        _ => [f32::MAX; 2],
    }
}

/// The item on the local player's cursor.
fn carried_id(host: &dyn ClientHost) -> Option<String> {
    let menu = host.local_player()?.container_menu(host)?;
    let stack = menu.carried(host).flatten().filter(|c| c.count() > 0)?;
    Some(item_id(host, &stack))
}

impl Shared {
    /// A press on trinket slot `c.slot` of the inventory screen, on the client tick: with nothing on the cursor, the
    /// worn stack comes off; with a stack that fits, it is parked in a free inventory slot (a click on the
    /// menu) and the server wears it from there. A press whose cursor changed meanwhile does nothing. A full
    /// inventory has no slot to park in, so the click is refused: only the server could swap with the cursor,
    /// and it cannot reach it (waits for sunrise-org/sunrise#4980).
    fn click_slot(&self, host: &mut dyn ClientHost, c: Click) {
        let i = c.slot;
        let Some(player) = host.local_player() else {
            return;
        };
        let Some(menu) = player.container_menu(&*host) else {
            return;
        };
        let carried = menu.carried(&*host).flatten().filter(|c| c.count() > 0);
        if carried.as_ref().map(|s| item_id(&*host, s)) != c.carried {
            return;
        }
        let click = match carried {
            None => {
                let view = lock(&self.view);
                let Some(Some(worn)) = view.get(i) else {
                    return;
                };
                SlotClick {
                    kind: TAKE,
                    slot: i,
                    inv: 0,
                    expect: item_id(&*host, worn),
                }
            }
            Some(stack) => {
                let id = item_id(&*host, &stack);
                if !accepts(i, &id) {
                    return;
                }
                // Menu slots 9..45 are the main inventory, then the hotbar.
                let Some(s) = (9..45).find(|&s| matches!(menu.slot(&*host, s), Some(None))) else {
                    return;
                };
                let pickup = MenuClick::Pickup {
                    slot: Some(s),
                    button: MouseButton::Left,
                };
                if !menu.click(host, &[pickup]) {
                    return;
                }
                SlotClick {
                    kind: WEAR,
                    slot: i,
                    inv: if s >= 36 { s - 36 } else { s },
                    expect: id,
                }
            }
        };
        if let Err(e) =
            host.send_payload_to_server(&PayloadId(SLOT.into()), PayloadBytes(click.encode()))
        {
            eprintln!("[trinkets] slot: {e:?}");
        }
    }
}

/// TrinketItem.use: the slot a right click with `stack` wears it in, if any.
fn wear_slot(
    host: &mut dyn Host,
    uuid: u128,
    stack: &ItemStack,
) -> Option<(usize, Vec<Option<ItemStack>>)> {
    let id = item_id(&*host, stack);
    // An elytra (anything equippable) still goes to its armour slot, as in Java: ElytraItem isn't a
    // TrinketItem.
    if !is_trinket(&id) || own_use(&id) || ItemResource::of(stack).equippable_slot(&*host).is_some()
    {
        return None;
    }
    let worn = worn(host, uuid);
    let i = (0..SLOTS.len()).find(|&i| worn[i].is_none() && accepts(i, &id))?;
    Some((i, worn))
}

/// A key name for a tooltip from its saved form (`key.keyboard.g`, `key.keyboard.left.shift`, `key.mouse.4`):
/// `G`, `Left Shift`, `Mouse 4`. None when unbound.
fn key_label(key: &str) -> Option<String> {
    let (kind, rest) = if let Some(r) = key.strip_prefix("key.keyboard.") {
        ("", r)
    } else {
        let r = key.strip_prefix("key.mouse.")?;
        ("Mouse ", r)
    };
    if rest.is_empty() || rest == "unknown" {
        return None;
    }
    let words: Vec<String> = rest
        .split('.')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect();
    Some(format!("{kind}{}", words.join(" ")))
}

/// The tooltip line for the slots `names` (the slots an item fits), with the key as the player bound it.
fn tooltip_line(names: &[&str], hint: &str) -> Option<String> {
    let mut names: Vec<&str> = names.to_vec();
    names.dedup();
    match names.as_slice() {
        [] => None,
        [one] => Some(format!("§7Equippable in the §9{one}§7 trinket slot{hint}")),
        many => Some(format!(
            "§7Equippable in the trinket slots: §9{}§7{hint}",
            many.join(", ")
        )),
    }
}

#[mod_api::sunrise_mod]
impl Hooks for Mod {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let codec = AttachmentCodec {
            encode: |c: &Compound| Ok(c.clone()),
            decode: |c: &Compound| Ok(c.clone()),
        };
        let descriptor = AttachmentDescriptor::new(WORN_ID, Compound::new)
            .serialize(codec)
            .sync(codec)
            .copy_on_death();
        match r.register_attachment_type(descriptor) {
            Ok(key) => *WORN.write().unwrap_or_else(|e| e.into_inner()) = Some(key),
            Err(e) => eprintln!("[trinkets] {WORN_ID} not registered: {e}"),
        }
        r.register_key_mapping(
            KeyRegistration::new(KEY, "key.keyboard.g").label("key.trinkets.open"),
        );
        r.menu_part(MENU, TrinketMenu(self.shared.clone()));
        r.payload_handlers()
            .registrar("1")
            .optional()
            .play_to_server(
                PayloadId(OPEN.into()),
                Arc::new(OpenHandler(self.shared.clone())),
            )
            .play_to_server(
                PayloadId(SLOT.into()),
                Arc::new(SlotHandler(self.shared.clone())),
            );
    }

    /// A player who joins may wear a totem saved with them.
    fn player_logged_in(&self, host: &mut dyn ServerHost, event: &mut PlayerLoggedInEvent<'_>) {
        self.shared.refresh_totem(host, event.player);
    }

    /// A respawned player is a new entity; the worn stacks came over with them.
    fn player_respawned(&self, host: &mut dyn ServerHost, event: &mut PlayerRespawnedEvent<'_>) {
        self.shared.refresh_totem(host, event.player);
    }

    /// `LivingEntity.checkTotemDeathProtection` for a totem worn in a trinket slot: once neither hand protects, the
    /// worn totem is the protector. One totem is used up (taken out of its slot, the attachment written and synced);
    /// the engine applies its death protection effects as a hand's totem would.
    fn living_death_protection(
        &self,
        host: &mut dyn HitHost,
        event: &mut LivingDeathProtectionEvent<'_>,
    ) {
        if event.protector.is_some() {
            return;
        }
        let Some(totem) = lock(&self.shared.totems).remove(&event.entity) else {
            return;
        };
        let Some(key) = worn_key() else {
            return;
        };
        let holder = Holder::Player(totem.uuid);
        let Some(mut c) = key
            .get_existing_in(host, holder, |c| c.cloned())
            .ok()
            .flatten()
        else {
            return;
        };
        let slot = SLOTS[totem.slot].0;
        let left = totem.stack.count().saturating_sub(1);
        match with_count(&totem.stack, left) {
            Some(rest) => {
                if let Some(Tag::Compound(entry)) = c.get_mut(&slot.into()) {
                    entry.insert("count".into(), Tag::Int(left as i32));
                }
                lock(&self.shared.totems).insert(
                    event.entity,
                    WornTotem {
                        uuid: totem.uuid,
                        slot: totem.slot,
                        stack: rest,
                    },
                );
            }
            None => {
                c.remove(&slot.into());
            }
        }
        if key.set_in(host, holder, c).is_err() {
            return;
        }
        *event.protector = with_count(&totem.stack, 1);
    }

    /// The Trinkets key, while playing or in the player's own inventory: asks the server for the menu.
    fn input_key(&self, host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
        if !matches!(event.action, KeyAction::Press) {
            return;
        }
        let bound = host
            .mod_key_mapping("trinkets", KEY)
            .ok()
            .and_then(|m| m.binding(&*host).ok().map(|(key, _)| key == event.key))
            == Some(true);
        if !bound {
            return;
        }
        let inventory = host
            .observed_screen_id()
            .is_some_and(|s| s.id == "inventory")
            && host.screen_title().is_none();
        if !(host.playing() || inventory) {
            return;
        }
        if let Err(e) =
            host.send_payload_to_server(&PayloadId(OPEN.into()), PayloadBytes(Vec::new()))
        {
            eprintln!("[trinkets] open: {e:?}");
        }
    }

    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        if !matches!(event.phase, ServerTickPhase::ServerTickEnd { .. }) {
            return;
        }
        let opens = std::mem::take(&mut *lock(&self.shared.opens));
        for uuid in opens {
            self.shared.open(host, uuid);
        }
        let slots = std::mem::take(&mut *lock(&self.shared.slots));
        for (uuid, click) in slots {
            self.shared.slot(host, uuid, click);
        }
    }

    /// While the player's inventory screen is open: reads the worn stacks for it to draw and does the clicks it took.
    fn client_tick_post(&self, host: &mut dyn ClientHost, _event: &mut ClientTickPostEvent) {
        let shown = host
            .observed_screen_id()
            .is_some_and(|s| s.id == "inventory")
            && host.screen_title().is_none();
        let clicks = std::mem::take(&mut *lock(&self.shared.clicks));
        let Some(player) = host.local_player().filter(|_| shown) else {
            // The screen is gone: its pending clicks and the presses it took go with it.
            *lock(&self.shared.presses) = Presses::default();
            return;
        };
        *lock(&self.shared.view) = worn(host, player.uuid());
        for i in clicks {
            self.shared.click_slot(host, i);
        }
    }

    /// The groups on the player's inventory screen: the first slot of each group that has no vanilla slot, and the
    /// popup of the hovered group (one framed row, its vanilla slot highlighted).
    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        if !player_inventory(event.screen) {
            return;
        }
        let (panel, mouse, gui) = (event.screen.rect, event.screen.mouse, gui_size(host));
        let worn = lock(&self.shared.view).clone();
        let open = {
            let mut open = lock(&self.shared.open);
            *open = open_group(panel, gui, mouse, *open);
            *open
        };
        let menu = host.local_player().and_then(|p| p.container_menu(host));
        let draw = &mut *event.draw;
        let slot = |draw: &mut dyn mod_api::DrawContext, i: usize, at: [f32; 2]| match worn
            .get(i)
            .cloned()
            .flatten()
        {
            Some(stack) => {
                draw.item_stack(&stack, at, 16.0);
            }
            None => {
                draw.sprite(
                    &icon(i),
                    [0.0, 0.0, 16.0, 16.0],
                    [at[0], at[1], 16.0, 16.0],
                    None,
                );
            }
        };
        for (g, group) in GROUPS.iter().enumerate() {
            if group.vanilla.is_none()
                && open != Some(g)
                && let Some(&i) = members(g).first()
            {
                let c = anchor_cell(panel, g);
                draw.sprite("container/slot", [0.0, 0.0, 18.0, 18.0], c, None);
                slot(draw, i, [panel[0] + group.at[0], panel[1] + group.at[1]]);
            }
            if open != Some(g) {
                continue;
            }
            let placed = place(panel, gui, g);
            // The frame: a cap, the stretch behind each slot, a cap.
            let frame = placed.frame();
            let n = ((frame[2] - 2.0 * FRAME_CAP) / CELL) as usize;
            let (x, y) = (frame[0], frame[1]);
            draw.sprite(
                "trinkets:popup_left",
                [0.0, 0.0, FRAME_CAP, FRAME_H],
                [x, y, FRAME_CAP, FRAME_H],
                None,
            );
            for k in 0..n {
                let dst = [x + FRAME_CAP + k as f32 * CELL, y, CELL, FRAME_H];
                draw.sprite("trinkets:popup_mid", [0.0, 0.0, CELL, FRAME_H], dst, None);
            }
            let right = [x + frame[2] - FRAME_CAP, y, FRAME_CAP, FRAME_H];
            draw.sprite(
                "trinkets:popup_right",
                [0.0, 0.0, FRAME_CAP, FRAME_H],
                right,
                None,
            );
            let first = placed.first(g);
            if let Some((index, empty)) = group.vanilla {
                match menu.and_then(|m| m.slot(host, index)).flatten() {
                    Some(stack) => {
                        draw.item_stack(&stack, first, 16.0);
                    }
                    None => {
                        draw.sprite(
                            empty,
                            [0.0, 0.0, 16.0, 16.0],
                            [first[0], first[1], 16.0, 16.0],
                            None,
                        );
                    }
                }
                draw.fill([first[0], first[1], 16.0, 16.0], 0x80FFFFFF);
            }
            for (i, at) in placed.cells() {
                slot(draw, i, at);
                if group.vanilla.is_none() && at == first {
                    draw.fill([at[0], at[1], 16.0, 16.0], 0x80FFFFFF);
                }
            }
        }
        // The tooltip: a worn item's own, or the slot's name.
        if let Some(i) = over(panel, gui, mouse, open) {
            let lines: Vec<Text> = match worn.get(i).cloned().flatten() {
                Some(stack) => ItemResource::of(&stack)
                    .tooltip(host, false, false)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|e| match e {
                        TooltipElement::Text(t, _) => Some(t),
                        _ => None,
                    })
                    .collect(),
                None => vec![Text::translate(
                    format!("trinkets.slot.{}", SLOTS[i].0.replace('/', ".")),
                    vec![],
                )],
            };
            let _ = draw.gui_tooltip(&lines, mouse);
        }
    }

    /// A press on one of the open group's slots belongs to the mod: the vanilla screen under it would take it for a
    /// click outside the panel and drop the cursor stack.
    fn screen_mouse_button_pressed_pre(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        let over = self.slot_under(host, event.screen);
        lock(&self.shared.presses).press(event.button, over.is_some());
        let Some(slot) = over else {
            return Flow::Continue;
        };
        if matches!(event.button, 0 | 1) {
            lock(&self.shared.clicks).push(Click {
                slot,
                carried: carried_id(host),
            });
        }
        Flow::Cancel
    }

    /// The release of a press the mod took is the mod's as well; any other release is the screen's, wherever the
    /// pointer is.
    fn screen_mouse_button_released_pre(
        &self,
        _host: &dyn ClientHost,
        event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
    ) -> Flow {
        if lock(&self.shared.presses).release(event.button) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// TrinketItem.use: a right click with a trinket wears it in the first empty slot that takes it. The server
    /// does the wearing; the client does not predict it.
    fn right_click_item(&self, host: HostSide<'_>, event: &mut RightClickItemEvent<'_>) -> Flow {
        let HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        let click = event.click;
        let Some((i, mut worn)) = wear_slot(host, click.player, click.stack) else {
            return Flow::Continue;
        };
        let Some(player) = host.player(click.player) else {
            return Flow::Continue;
        };
        let hand_slot = match click.hand {
            Hand::Main => player.selected_slot(&*host).map(|s| s as u32),
            _ => Some(40),
        };
        let Some(hand_slot) = hand_slot else {
            return Flow::Continue;
        };
        let remainder = with_count(click.stack, click.stack.count().saturating_sub(1));
        if player
            .set_inventory_slot(host, hand_slot, remainder)
            .is_err()
        {
            return Flow::Continue;
        }
        worn[i] = with_count(click.stack, 1);
        set_worn(host, click.player, &worn);
        self.shared.refresh_totem(host, click.player);
        if let Some(entity) = host.player(click.player).map(|p| p.entity()) {
            entity.play_sound(host, "minecraft:item.armor.equip_generic", 1.0, 1.0);
        }
        *event.cancellation_result = InteractionResult::Success;
        event.cancelled = true;
        Flow::Cancel
    }

    /// TrinketsApi's death drop (`dropInventory`): unless keepInventory, the worn stacks drop where the player
    /// died, vanishing-cursed ones destroyed. With keepInventory they stay worn.
    fn living_death(&self, host: &mut dyn ServerHost, event: &mut LivingDeathEvent<'_>) -> Flow {
        if event.kind != "minecraft:player" {
            return Flow::Continue;
        }
        let Some(entity) = host.entity(event.entity) else {
            return Flow::Continue;
        };
        let Some(uuid) = entity.uuid(host) else {
            return Flow::Continue;
        };
        let reader: &dyn ServerHost = &*host;
        let Some(level) = entity.level(reader) else {
            return Flow::Continue;
        };
        let keep = matches!(
            level.game_rule(reader, "minecraft:keep_inventory"),
            Ok(Some(mod_api::GameRuleValue::Bool(true)))
        );
        if keep {
            return Flow::Continue;
        }
        let Some(at) = entity.position(reader) else {
            return Flow::Continue;
        };
        let dim = host
            .player(uuid)
            .and_then(|p| p.dimension(&*host))
            .map(|d| d.to_string());
        let (Some(dim), stacks) = (dim, worn(host, uuid)) else {
            return Flow::Continue;
        };
        if stacks.iter().all(Option::is_none) {
            return Flow::Continue;
        }
        // Unknown stand-ins and every other saved entry stay as they are.
        let kept: Vec<Option<ItemStack>> = stacks
            .iter()
            .map(|s| s.clone().filter(is_unknown))
            .collect();
        set_worn(host, uuid, &kept);
        let Some(level) = host.level(&dim) else {
            return Flow::Continue;
        };
        let pos = [
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        ];
        for stack in stacks.into_iter().flatten().filter(|s| !is_unknown(s)) {
            if !has_vanishing_curse(&stack) {
                let _ = level.pop_resource(host, pos, stack);
            }
        }
        Flow::Continue
    }

    /// TrinketsClient's tooltip: which trinket slots take the item (placeholders show nothing).
    fn item_tooltip(&self, host: &dyn ClientHost, event: &mut ItemTooltipEvent<'_>) {
        let Some(id) = event.stack.item().id(host) else {
            return;
        };
        if id.starts_with("trinkets:slot_") {
            return;
        }
        let names: Vec<&str> = SLOTS
            .iter()
            .filter(|s| s.3.contains(&id))
            .map(|s| s.1)
            .collect();
        // The key as the player bound it; no hint when it's unbound.
        let hint = host
            .mod_key_mapping("trinkets", KEY)
            .ok()
            .and_then(|m| m.binding(host).ok())
            .and_then(|(key, _)| key_label(key))
            .map(|k| format!(" ({k})"))
            .unwrap_or_default();
        if let Some(line) = tooltip_line(&names, &hint) {
            let _ = event.lines.push(Text::literal(line), 0xFFAAAAAA);
        }
    }
}

impl Mod {
    /// The trinket slot the pointer is on, on the player's inventory screen.
    fn slot_under(&self, host: &dyn ClientHost, screen: &ScreenView<'_>) -> Option<usize> {
        if !player_inventory(screen) {
            return None;
        }
        let gui = gui_size(host);
        let open = open_group(screen.rect, gui, screen.mouse, *lock(&self.shared.open));
        over(screen.rect, gui, screen.mouse, open)
    }
}

#[derive(Default)]
struct Mod {
    shared: Arc<Shared>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_worn_totem_protects() {
        assert_eq!(
            first_totem(&[None, Some("minecraft:apple"), Some(TOTEM), Some(TOTEM)]),
            Some(2)
        );
        assert_eq!(first_totem(&[None, Some("minecraft:apple")]), None);
        assert_eq!(first_totem(&[]), None);
    }

    #[test]
    fn key_labels() {
        assert_eq!(key_label("key.keyboard.g").as_deref(), Some("G"));
        assert_eq!(
            key_label("key.keyboard.left.shift").as_deref(),
            Some("Left Shift")
        );
        assert_eq!(
            key_label("key.keyboard.keypad.1").as_deref(),
            Some("Keypad 1")
        );
        assert_eq!(key_label("key.mouse.4").as_deref(), Some("Mouse 4"));
        assert_eq!(key_label("key.keyboard.unknown"), None);
        assert_eq!(key_label("key.keyboard."), None);
        assert_eq!(key_label("g"), None);
    }

    #[test]
    fn inventory_slots_follow_the_menu() {
        let size = rows() as usize * 9;
        assert_eq!(inventory_slot(size, size as i32), Some(9));
        assert_eq!(inventory_slot(size, size as i32 + 26), Some(35));
        assert_eq!(inventory_slot(size, size as i32 + 27), Some(0));
        assert_eq!(inventory_slot(size, size as i32 + 35), Some(8));
        assert_eq!(inventory_slot(size, size as i32 + 36), None);
        assert_eq!(inventory_slot(size, 3), None);
        assert_eq!(inventory_slot(size, -999), None);
    }

    #[test]
    fn vanishing_curse_is_read_from_the_enchantments() {
        let mut enchantments = Compound::new();
        enchantments.insert("minecraft:vanishing_curse".into(), Tag::Int(1));
        let mut components = Compound::new();
        assert!(!vanishes(&components));
        components.insert("minecraft:enchantments".into(), Tag::Compound(enchantments));
        assert!(vanishes(&components));
        let mut other = Compound::new();
        other.insert(
            "minecraft:enchantments".into(),
            Tag::Compound(Compound::new()),
        );
        assert!(!vanishes(&other));
    }

    #[test]
    fn the_menu_is_two_rows() {
        assert_eq!(SLOTS.len(), 15);
        assert_eq!(rows(), 2);
        assert_eq!(MENU, "minecraft:generic_9x2");
    }

    #[test]
    fn every_slot_is_in_one_group() {
        let mut seen: Vec<usize> = (0..GROUPS.len())
            .flat_map(|g| members(g).iter().copied())
            .collect();
        seen.sort();
        assert_eq!(seen, (0..SLOTS.len()).collect::<Vec<_>>());
    }

    #[test]
    fn positions_alternate_right_then_left() {
        let order: Vec<i32> = (0..10).map(popup::offset).collect();
        assert_eq!(order, [0, 1, -1, 2, -2, 3, -3, 4, -4, 5]);
    }

    const WIDE: [f32; 2] = [1000.0, 600.0];

    #[test]
    fn a_group_is_one_row_over_its_vanilla_slot() {
        let panel = [100.0, 50.0, 176.0, 166.0];
        let chest = GROUPS.iter().position(|g| g.id == "chest").unwrap();
        let placed = place(panel, WIDE, chest);
        let cells: Vec<_> = placed.cells().collect();
        // Necklace right of the chest slot, cape left of it, then back and charm further out.
        let x = |n: usize| cells[n].1[0];
        assert_eq!(
            cells[0].0,
            SLOTS.iter().position(|s| s.0 == "chest/necklace").unwrap()
        );
        assert_eq!(x(0), panel[0] + 8.0 + CELL);
        assert_eq!(x(1), panel[0] + 8.0 - CELL);
        // Five positions in the frame, the vanilla slot among them.
        assert_eq!(placed.frame()[2], 5.0 * CELL + 2.0 * FRAME_CAP);
        // The pointer on the vanilla slot opens the group; it stays open over the row, not elsewhere.
        let on_slot = [panel[0] + 10.0, panel[1] + 28.0];
        assert_eq!(open_group(panel, WIDE, on_slot, None), Some(chest));
        let on_necklace = [x(0) + 3.0, panel[1] + 28.0];
        assert_eq!(open_group(panel, WIDE, on_necklace, None), None);
        assert_eq!(
            open_group(panel, WIDE, on_necklace, Some(chest)),
            Some(chest)
        );
        assert_eq!(
            over(panel, WIDE, on_necklace, Some(chest)),
            Some(cells[0].0)
        );
    }

    #[test]
    fn groups_without_a_vanilla_slot_start_on_their_first_slot() {
        let panel = [0.0, 0.0, 176.0, 166.0];
        let hand = GROUPS.iter().position(|g| g.id == "hand").unwrap();
        let cells: Vec<_> = place(panel, WIDE, hand).cells().collect();
        assert_eq!(cells[0].1, [77.0, 44.0]);
        assert_eq!(cells[1].1, [77.0 + CELL, 44.0]);
    }

    #[test]
    fn a_popup_leaving_the_screen_flips_or_is_pushed_back() {
        let chest = GROUPS.iter().position(|g| g.id == "chest").unwrap();
        let inside = |p: &popup::Placed, w: f32| {
            let f = p.frame();
            f[0] >= 0.0 && f[0] + f[2] <= w
        };
        // Panel hard against the left edge, five positions either way: pushed back in.
        let placed = place([0.0, 50.0, 176.0, 166.0], [400.0, 300.0], chest);
        assert!(inside(&placed, 400.0));
        // A screen narrower than the popup: pushed to the left edge.
        let placed = place([10.0, 50.0, 176.0, 166.0], [60.0, 300.0], chest);
        assert_eq!(placed.frame()[0], 0.0);
        // The offhand's two positions: the ring goes right of the vanilla slot, left when the right is off the screen.
        let offhand = GROUPS.iter().position(|g| g.id == "offhand").unwrap();
        let ring = |panel_x: f32| place([panel_x, 50.0, 176.0, 166.0], [400.0, 300.0], offhand);
        let (near, far) = (ring(100.0), ring(300.0));
        assert_eq!(near.cells().next().unwrap().1[0], 100.0 + 77.0 + CELL);
        assert_eq!(far.cells().next().unwrap().1[0], 300.0 + 77.0 - CELL);
        assert!(inside(&near, 400.0) && inside(&far, 400.0));
    }

    #[test]
    fn display_order_is_not_equip_order() {
        // Every slot shown once.
        let mut shown: Vec<&str> = data::DISPLAY.to_vec();
        shown.sort();
        let mut all: Vec<&str> = SLOTS.iter().map(|s| s.0).collect();
        all.sort();
        assert_eq!(shown, all);
        // The popup puts the cape next to the chest slot, ahead of the back slot ...
        let chest = GROUPS.iter().position(|g| g.id == "chest").unwrap();
        let ids: Vec<&str> = members(chest).iter().map(|&i| SLOTS[i].0).collect();
        assert_eq!(
            ids,
            ["chest/necklace", "chest/cape", "chest/back", "chest/charm"]
        );
        // ... but an item both take goes to the back slot first, as it did before the popup.
        let first = (0..SLOTS.len()).find(|&i| accepts(i, "vc_gliders:paraglider_wood"));
        assert_eq!(SLOTS[first.unwrap()].0, "chest/back");
        let equip: Vec<&str> = SLOTS
            .iter()
            .map(|s| s.0)
            .filter(|s| s.starts_with("chest/"))
            .collect();
        assert_eq!(
            equip,
            ["chest/necklace", "chest/back", "chest/cape", "chest/charm"]
        );
    }

    #[test]
    fn only_the_release_of_a_taken_press_is_taken() {
        let mut p = Presses::default();
        // A press the mod took: its release is the mod's, wherever the pointer is.
        p.press(0, true);
        assert!(p.release(0));
        // The end of a vanilla gesture is not, even over a trinket slot.
        p.press(0, false);
        assert!(!p.release(0));
        // Taken once, released once.
        p.press(1, true);
        assert!(!p.release(0));
        assert!(p.release(1));
        assert!(!p.release(1));
        // A new press of the button ends the taken one before it.
        p.press(0, true);
        p.press(0, false);
        assert!(!p.release(0));
        // Odd button numbers take nothing.
        p.press(-1, true);
        p.press(40, true);
        assert!(!p.release(-1) && !p.release(40));
    }

    #[test]
    fn slot_clicks_survive_the_wire() {
        let c = SlotClick {
            kind: WEAR,
            slot: 3,
            inv: 35,
            expect: "minecraft:totem_of_undying".into(),
        };
        assert_eq!(SlotClick::decode(&c.encode()), Some(c));
        assert_eq!(SlotClick::decode(&[WEAR, 99, 0]), None);
        assert_eq!(SlotClick::decode(&[WEAR, 0, 36]), None);
        assert_eq!(SlotClick::decode(&[7, 0, 0]), None);
        assert_eq!(SlotClick::decode(&[TAKE, 0]), None);
    }

    /// Slots, worn stacks and the give of a click, with a step that fails.
    #[derive(Default)]
    struct Fake {
        slots: Vec<Option<u8>>,
        worn: Vec<Option<u8>>,
        given: Vec<u8>,
        fail: &'static str,
    }

    impl Store<u8> for Fake {
        fn set_slot(&mut self, slot: u32, stack: Option<u8>) -> bool {
            // A failing slot write changes nothing, like the game's.
            if self.fail == "slot" && stack.is_some() != self.slots[slot as usize].is_some() {
                return false;
            }
            self.slots[slot as usize] = stack;
            true
        }
        fn set_worn(&mut self, worn: &[Option<u8>]) -> bool {
            if self.fail == "worn" && worn.iter().flatten().count() == 2 {
                return false;
            }
            self.worn = worn.to_vec();
            true
        }
        fn give(&mut self, stack: u8) -> bool {
            if self.fail == "give" {
                return false;
            }
            self.given.push(stack);
            true
        }
    }

    #[test]
    fn a_failed_step_leaves_nothing_changed() {
        // Wear item 7 from slot 0 (a stack of one) in place of worn item 5.
        let old = vec![Some(5), None];
        let new = vec![Some(7), None];
        let slot = Some((0, None, Some(7)));
        let start = |fail| Fake {
            slots: vec![Some(7)],
            worn: old.clone(),
            fail,
            ..Default::default()
        };
        let mut ok = start("");
        assert!(commit(&mut ok, slot.clone(), &old, &new, Some(5)));
        assert_eq!(
            (ok.slots, ok.worn, ok.given),
            (vec![None], new.clone(), vec![5])
        );
        // The give is refused: the worn stacks and the inventory slot are as they were.
        let mut f = start("give");
        assert!(!commit(&mut f, slot.clone(), &old, &new, Some(5)));
        assert_eq!(
            (f.slots, f.worn, f.given),
            (vec![Some(7)], old.clone(), vec![])
        );
        // The attachment write is refused: the inventory slot is as it was and nothing was given.
        let two = vec![Some(7), Some(5)];
        let mut f = start("worn");
        assert!(!commit(&mut f, slot.clone(), &old, &two, Some(5)));
        assert_eq!(
            (f.slots, f.worn, f.given),
            (vec![Some(7)], old.clone(), vec![])
        );
        // The inventory write is refused: nothing else was written.
        let mut f = start("slot");
        assert!(!commit(&mut f, slot, &old, &new, Some(5)));
        assert_eq!(
            (f.slots, f.worn, f.given),
            (vec![Some(7)], old.clone(), vec![])
        );
    }

    #[test]
    fn backpacks_open_rather_than_wear() {
        assert!(own_use("sophisticatedbackpacks:iron_backpack"));
        assert!(!own_use("things:socks"));
    }

    #[test]
    fn the_cape_takes_an_elytra() {
        let cape = SLOTS.iter().position(|s| s.0 == "chest/cape").unwrap();
        assert!(accepts(cape, "minecraft:elytra"));
        assert!(!accepts(cape, "minecraft:stick"));
        assert!(!accepts(999, "minecraft:elytra"));
        assert!(is_trinket("minecraft:totem_of_undying"));
        assert!(!is_trinket("minecraft:stick"));
    }

    #[test]
    fn the_back_slot_takes_backpacks() {
        let back = SLOTS.iter().position(|s| s.0 == "chest/back").unwrap();
        assert!(accepts(back, "sophisticatedbackpacks:iron_backpack"));
    }

    #[test]
    fn slot_ids_are_unique_and_have_placeholders() {
        for (n, s) in SLOTS.iter().enumerate() {
            assert!(SLOTS[n + 1..].iter().all(|t| t.0 != s.0), "{}", s.0);
            assert!(s.2.starts_with("trinkets:slot_"));
            assert_eq!(s.2, format!("trinkets:slot_{}", s.0.replace('/', "_")));
        }
    }

    #[test]
    fn wearing_fills_an_empty_slot_first() {
        let mut occupied = vec![false; SLOTS.len()];
        occupied[1] = true;
        assert_eq!(
            plan_wear(&[1, 4], &occupied, 1, false, 7),
            Plan::Wear { i: 4, inv: 7 }
        );
    }

    #[test]
    fn wearing_swaps_when_every_fit_is_taken() {
        let occupied = vec![true; SLOTS.len()];
        assert_eq!(
            plan_wear(&[2, 3], &occupied, 1, false, 0),
            Plan::Wear { i: 2, inv: 0 }
        );
        // A stack of several needs room for what comes back.
        assert_eq!(
            plan_wear(&[2], &occupied, 5, false, 0),
            Plan::Refuse("No room in your inventory")
        );
        assert_eq!(
            plan_wear(&[2], &occupied, 5, true, 0),
            Plan::Wear { i: 2, inv: 0 }
        );
    }

    #[test]
    fn wearing_needs_a_slot_that_takes_it() {
        assert_eq!(
            plan_wear(&[], &vec![false; SLOTS.len()], 1, true, 0),
            Plan::Refuse("That doesn't go in a trinket slot")
        );
    }

    #[test]
    fn tooltips_name_the_slots() {
        assert_eq!(
            tooltip_line(&["Cape"], " (G)").as_deref(),
            Some("§7Equippable in the §9Cape§7 trinket slot (G)")
        );
        assert_eq!(
            tooltip_line(&["Ring", "Ring", "Belt"], "").as_deref(),
            Some("§7Equippable in the trinket slots: §9Ring, Belt§7")
        );
        assert_eq!(tooltip_line(&[], ""), None);
    }
}
