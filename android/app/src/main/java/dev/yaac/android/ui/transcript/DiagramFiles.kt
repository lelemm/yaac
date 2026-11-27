package dev.yaac.android.ui.transcript

import java.io.File
import java.util.UUID

/** Saving a mermaid diagram (T15): PNG, SVG or its source, through the system document picker. */
object DiagramFiles {
    /** The suggested name, as the desktop's: `diagram.png`, `diagram.svg`, `diagram.mmd`. */
    fun name(extension: String) = "diagram.$extension"

    /** The type filter the picker is given (`PlatformBridge.saveFile`). */
    fun filter(extension: String) = when (extension) {
        "png" -> "PNG image"
        "svg" -> "SVG image"
        else -> "Mermaid source"
    }

    /**
     * The drawn `<svg>` element as a standalone file: the XML declaration, and the SVG
     * namespace when the page's serialisation left it out. Null when it is not an SVG.
     */
    fun svgDocument(markup: String?): String? {
        val svg = markup?.trim()?.takeIf { it.startsWith("<svg") } ?: return null
        val head = svg.substringBefore('>')
        val withNs = if ("xmlns=" in head) svg else "<svg xmlns=\"http://www.w3.org/2000/svg\"" + svg.removePrefix("<svg")
        return "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n$withNs"
    }

    /** Writes [bytes] under [cache] for the picker to copy from; the file the picker reads. */
    fun stage(cache: File, extension: String, bytes: ByteArray): File {
        val dir = File(cache, "save/${UUID.randomUUID()}").apply { mkdirs() }
        return File(dir, name(extension)).apply { writeBytes(bytes) }
    }
}
