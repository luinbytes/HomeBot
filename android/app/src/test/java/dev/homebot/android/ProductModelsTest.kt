package dev.homebot.android

import dev.homebot.protocol.BotAdvancedSettings
import dev.homebot.protocol.BotSummary
import dev.homebot.protocol.ChatSummary
import org.junit.Assert.assertEquals
import org.junit.Test

class ProductModelsTest {
    @Test
    fun `conversation order keeps pins first then uses recent activity`() {
        val bots = listOf(bot("quiet", "Quiet"), bot("new", "New"), bot("pin", "Pinned", pinned = true))
        val chats = listOf(chat("quiet", 4), chat("new", 9), chat("pin", 1))

        assertEquals(listOf("pin", "new", "quiet"), botConversations(bots, chats).map { it.bot.id })
    }

    private fun bot(id: String, name: String, pinned: Boolean = false) = BotSummary(
        id = id,
        name = name,
        title = "Assistant",
        description = "",
        shape = "circle",
        color = "violet",
        archived = false,
        pinned = pinned,
        hidden = false,
        unread_count = 0,
        attention = "none",
        provider = "test",
        advanced = BotAdvancedSettings(null, "ask_before_changes"),
    )

    private fun chat(botId: String, sequence: Long) = ChatSummary(
        id = "chat-$botId",
        title = "Chat with $botId",
        bot_id = botId,
        unread_count = 0,
        running = false,
        queued_count = 0,
        last_sequence = sequence,
    )
}
