package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals

class AssistantSectionsTest {
    @Test
    fun liftsAnalysisAndUnwrapsSummaryInOrder() {
        val sections = AssistantSections.parse(
            "Before\n<analysis>\n**thought**\n</analysis>\n" +
                "<summary>\n**answer**\n</summary>\nAfter",
        )

        assertEquals(
            listOf(
                AssistantSection(AssistantSectionKind.NORMAL, "Before"),
                AssistantSection(AssistantSectionKind.ANALYSIS, "**thought**"),
                AssistantSection(AssistantSectionKind.NORMAL, "**answer**"),
                AssistantSection(AssistantSectionKind.NORMAL, "After"),
            ),
            sections,
        )
    }

    @Test
    fun keepsWrapperExamplesInsideFencedCodeLiteral() {
        val text = "```text\n<analysis>\ninside\n</analysis>\n```"
        assertEquals(
            listOf(AssistantSection(AssistantSectionKind.NORMAL, text)),
            AssistantSections.parse(text),
        )
    }

    @Test
    fun leavesMalformedWrappersLiteral() {
        val text = "<analysis>\nunfinished\n<summary>\nanswer\n</summary>"
        assertEquals(
            listOf(AssistantSection(AssistantSectionKind.NORMAL, text)),
            AssistantSections.parse(text),
        )
    }

    @Test
    fun hidesAnalysisFromLiveProjectionAndKeepsSummaryVisible() {
        val text = "<analysis>\nthinking\n</analysis>\n<summary>\nanswer\n</summary>"
        assertEquals("answer", AssistantSections.stream(text))
    }

    @Test
    fun withholdsPartialWrapperTagsWhileStreaming() {
        assertEquals("Before", AssistantSections.stream("Before\n<ana"))
        assertEquals("answer", AssistantSections.stream("<summary>\nanswer\n</sum"))
    }
}
