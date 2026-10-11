package app.porchlight.shared.live

import org.freedesktop.dbus.DBusPath
import org.freedesktop.dbus.Struct
import org.freedesktop.dbus.Tuple
import org.freedesktop.dbus.annotations.*
import org.freedesktop.dbus.errors.*
import org.freedesktop.dbus.interfaces.*
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.*

/** Application-owned contribution point. Features register here, never create another tray/bus. */
class DesktopTrayActions {
    data class Entry(val id: Int, val label: String, val enabled: Boolean, val invoke: () -> Unit)
    private val entries = linkedMapOf<String, Entry>()
    private val ids = mutableMapOf<String, Int>()
    private var next = 3
    @Volatile internal var changed: () -> Unit = {}
    fun register(key: String, label: String, enabled: Boolean = true, invoke: () -> Unit) {
        require(key.isNotBlank() && label.isNotBlank())
        synchronized(this) { entries[key] = Entry(ids.getOrPut(key) { next++ }, label, enabled, invoke) }
        changed()
    }
    fun unregister(key: String) { if (synchronized(this) { entries.remove(key) != null }) changed() }
    @Synchronized internal fun snapshot() = entries.values.toList()
    @Synchronized internal fun entry(key: String) = entries[key]
}

@DBusInterfaceName("org.kde.StatusNotifierItem")
@DBusProperties(value = [
    DBusProperty(name = "Category", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "Id", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "Title", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "Status", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "WindowId", type = UInt32::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "IconName", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "IconPixmap", type = Array<TrayPixmap>::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "OverlayIconName", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "OverlayIconPixmap", type = Array<TrayPixmap>::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "AttentionIconName", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "AttentionIconPixmap", type = Array<TrayPixmap>::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "AttentionMovieName", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "ToolTip", type = TrayTooltip::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "ItemIsMenu", type = Boolean::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "Menu", type = DBusPath::class, access = DBusProperty.Access.READ),
])
interface TrayItem : DBusInterface {
    fun Activate(x: Int, y: Int)
    fun SecondaryActivate(x: Int, y: Int)
    fun ContextMenu(x: Int, y: Int)
    fun Scroll(delta: Int, orientation: String)
    class NewStatus(path: String, val status: String) : DBusSignal(path, status)
    class NewIcon(path: String) : DBusSignal(path)
    class NewToolTip(path: String) : DBusSignal(path)
}
class TrayTooltip(@Position(0) val icon: String, @Position(1) val pixmaps: List<TrayPixmap>,
    @Position(2) val title: String, @Position(3) val description: String) : Struct()
class TrayPixmap(@Position(0) val width: Int, @Position(1) val height: Int, @Position(2) val data: ByteArray) : Struct()
class MenuNode(@Position(0) val id: Int, @Position(1) val properties: Map<String, Variant<*>>,
    @Position(2) val children: List<Variant<*>>) : Struct()
class MenuLayout(@Position(0) val revision: UInt32, @Position(1) val layout: MenuNode) : Tuple()
class MenuProperties(@Position(0) val id: Int, @Position(1) val properties: Map<String, Variant<*>>) : Struct()
class MenuRemoved(@Position(0) val id: Int, @Position(1) val properties: List<String>) : Struct()
class MenuEvent(@Position(0) val id: Int, @Position(1) val event: String,
    @Position(2) val data: Variant<*>, @Position(3) val timestamp: UInt32) : Struct()
class MenuShowResult(@Position(0) val updates: IntArray, @Position(1) val errors: IntArray) : Tuple()
@DBusInterfaceName("com.canonical.dbusmenu")
@DBusProperties(value = [
    DBusProperty(name = "Version", type = UInt32::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "TextDirection", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "Status", type = String::class, access = DBusProperty.Access.READ),
    DBusProperty(name = "IconThemePath", type = Array<String>::class, access = DBusProperty.Access.READ),
])
interface TrayMenu : DBusInterface {
    fun GetLayout(parentId: Int, recursionDepth: Int, propertyNames: List<String>): MenuLayout
    fun GetGroupProperties(ids: List<Int>, propertyNames: List<String>): List<MenuProperties>
    fun GetProperty(id: Int, name: String): Variant<*>
    fun Event(id: Int, eventId: String, data: Variant<*>, timestamp: UInt32)
    fun EventGroup(events: List<MenuEvent>): List<Int>
    fun AboutToShow(id: Int): Boolean
    fun AboutToShowGroup(ids: List<Int>): MenuShowResult
    class LayoutUpdated(path: String, val revision: UInt32, val parent: Int) : DBusSignal(path, revision, parent)
    class ItemsPropertiesUpdated(path: String, val updated: List<MenuProperties>, val removed: List<MenuRemoved>) : DBusSignal(path, updated, removed)
}

abstract class ReadOnlyBusProperties(private val interfaceName: String) : Properties {
    protected abstract fun properties(): Map<String, Variant<*>>
    override fun GetAll(interfaceName: String): Map<String, Variant<*>> {
        if (interfaceName != this.interfaceName) throw UnknownInterface("Unknown interface")
        return properties()
    }
    @Suppress("UNCHECKED_CAST") override fun <A> Get(interfaceName: String, propertyName: String): A =
        (GetAll(interfaceName)[propertyName] ?: throw UnknownProperty("Unknown property")).value as A
    override fun <A> Set(interfaceName: String, propertyName: String, value: A) { throw PropertyReadOnly("Read-only property") }
}

internal class DesktopTray(private val open: () -> Unit, private val actions: DesktopTrayActions,
    private val emit: (DBusSignal) -> Unit) : ReadOnlyBusProperties("org.kde.StatusNotifierItem"), TrayItem {
    private var status = "Active"
    private var description = "Household connected"
    private var icon = "porchlight"
    private val appPixmaps = listOf(porchlightTrayPixmap())
    override fun getObjectPath() = "/StatusNotifierItem"
    override fun Activate(x: Int, y: Int) = open()
    override fun SecondaryActivate(x: Int, y: Int) = open()
    override fun ContextMenu(x: Int, y: Int) {} // Host renders Menu; do not unexpectedly open the window.
    override fun Scroll(delta: Int, orientation: String) {}
    @Synchronized fun update(unreachable: Boolean, unread: Int) {
        val next = if (unreachable || unread > 0) "NeedsAttention" else "Active"
        val text = if (unreachable) "Can't reach home" else if (unread > 0) "$unread unread household alerts" else "Household connected"
        val nextIcon = "porchlight"
        if (next == status && text == description && nextIcon == icon) return
        if (next != status) { status = next; emit(TrayItem.NewStatus(objectPath, status)) }
        if (nextIcon != icon) { icon = nextIcon; emit(TrayItem.NewIcon(objectPath)) }
        if (text != description) { description = text; emit(TrayItem.NewToolTip(objectPath)) }
        emit(Properties.PropertiesChanged(objectPath, "org.kde.StatusNotifierItem", properties(), emptyList()))
    }
    @Synchronized override fun properties(): Map<String, Variant<*>> = mapOf(
        "Category" to Variant("ApplicationStatus"), "Id" to Variant("porchlight"), "Title" to Variant("Porchlight"),
        "Status" to Variant(status), "IconName" to Variant(icon), "AttentionIconName" to Variant("porchlight"),
        "IconPixmap" to Variant(appPixmaps, "a(iiay)"), "AttentionIconPixmap" to Variant(appPixmaps, "a(iiay)"),
        "OverlayIconName" to Variant(""), "OverlayIconPixmap" to Variant(emptyList<TrayPixmap>(), "a(iiay)"),
        "AttentionMovieName" to Variant(""), "ItemIsMenu" to Variant(false), "WindowId" to Variant(UInt32(0)),
        "Menu" to Variant(DBusPath("/Menu")), "ToolTip" to Variant(TrayTooltip(icon, appPixmaps, "Porchlight", description), "(sa(iiay)ss)"))
}

internal class DesktopMenu(open: () -> Unit, quit: () -> Unit, private val actions: DesktopTrayActions,
    private val emit: (DBusSignal) -> Unit) : ReadOnlyBusProperties("com.canonical.dbusmenu"), TrayMenu {
    private val builtins = listOf(DesktopTrayActions.Entry(1, "Open Porchlight", true, open), DesktopTrayActions.Entry(2, "Quit", true, quit))
    private var revision = 1L
    init { actions.changed = { synchronized(this) { revision = if (revision == 0xffffffffL) 1 else revision + 1; emit(TrayMenu.LayoutUpdated(objectPath, UInt32(revision), 0)) } } }
    override fun getObjectPath() = "/Menu"
    private fun entries() = builtins.take(1) + actions.snapshot() + builtins.takeLast(1)
    private fun props(id: Int): Map<String, Variant<*>> {
        if (id == 0) return mapOf("children-display" to Variant("submenu"))
        val entry = entries().firstOrNull { it.id == id } ?: throw UnknownObject("Unknown menu item")
        return mapOf("label" to Variant(entry.label), "enabled" to Variant(entry.enabled), "visible" to Variant(true),
            "type" to Variant("standard"), "children-display" to Variant(""), "icon-name" to Variant(""),
            "toggle-type" to Variant(""), "toggle-state" to Variant(-1), "shortcut" to Variant(emptyList<List<String>>(), "aas"), "disposition" to Variant("normal"))
    }
    private fun filtered(id: Int, names: List<String>) = props(id).filterKeys { names.isEmpty() || it in names }
    @Synchronized override fun GetLayout(parentId: Int, recursionDepth: Int, propertyNames: List<String>): MenuLayout {
        if (recursionDepth < -1) throw InvalidMethodArgument("Invalid depth")
        val children = if (parentId == 0 && recursionDepth != 0) entries().map { entry ->
            // Explicit signature is essential: empty children/maps cannot infer their element types.
            Variant(MenuNode(entry.id, filtered(entry.id, propertyNames), emptyList()), "(ia{sv}av)")
        } else emptyList()
        return MenuLayout(UInt32(revision), MenuNode(parentId, filtered(parentId, propertyNames), children))
    }
    override fun GetGroupProperties(ids: List<Int>, propertyNames: List<String>): List<MenuProperties> =
        (if (ids.isEmpty()) listOf(0) + entries().map { it.id } else ids).mapNotNull { id -> runCatching { MenuProperties(id, filtered(id, propertyNames)) }.getOrNull() }
    override fun GetProperty(id: Int, name: String) = props(id)[name] ?: throw UnknownProperty("Unknown menu property")
    override fun Event(id: Int, eventId: String, data: Variant<*>, timestamp: UInt32) {
        props(id)
        if (eventId == "clicked") entries().firstOrNull { it.id == id && it.enabled }?.invoke?.invoke()
    }
    override fun EventGroup(events: List<MenuEvent>) = events.mapNotNull { event ->
        try { Event(event.id, event.event, event.data, event.timestamp); null } catch (_: UnknownObject) { event.id }
    }
    override fun AboutToShow(id: Int): Boolean { props(id); return false }
    override fun AboutToShowGroup(ids: List<Int>) = MenuShowResult(intArrayOf(), ids.filter { id -> runCatching { props(id) }.isFailure }.toIntArray())
    override fun properties(): Map<String, Variant<*>> = mapOf("Version" to Variant(UInt32(3)), "TextDirection" to Variant("ltr"), "Status" to Variant("normal"), "IconThemePath" to Variant(emptyList<String>(), "as"))
    fun close() { actions.changed = {} }
}

/** SNI ARGB bytes are network-order; use the same branded asset as the window/launcher. */
@OptIn(org.jetbrains.compose.resources.ExperimentalResourceApi::class)
internal fun porchlightTrayPixmap(): TrayPixmap {
    val png = kotlinx.coroutines.runBlocking { app.porchlight.shared.resources.Res.readBytes("drawable/porchlight_icon.png") }
    val image = javax.imageio.ImageIO.read(java.io.ByteArrayInputStream(png))
    val size = 32
    val scaled = java.awt.image.BufferedImage(size, size, java.awt.image.BufferedImage.TYPE_INT_ARGB)
    scaled.createGraphics().let { g -> try { g.drawImage(image, 0, 0, size, size, null) } finally { g.dispose() } }
    val pixels = scaled.getRGB(0, 0, size, size, null, 0, size)
    val bytes = java.nio.ByteBuffer.allocate(size * size * 4)
    pixels.forEach { bytes.putInt(it) }
    return TrayPixmap(size, size, bytes.array())
}
