package com.restartfu.xd.syntax

import com.restartfu.xd.syntax.SyntaxLanguage as L

/**
 * Line scanner for the languages the desktop highlights.
 *
 * This covers the constructs code actually spends its time in: comments,
 * strings, numbers, keywords, types, constants and call sites. It deliberately
 * does not attempt the desktop's exotic lexing -- heredocs, Ruby and Crystal
 * percent literals and regex, C# verbatim and raw strings, Bash expansion and
 * quoting state, Crystal macro delimiters, TOML table headers, YAML anchors.
 * Those fall back to plain text, which reads as uncoloured rather than
 * mis-coloured; see docs/mobile.md.
 */
internal object Scanner {
    fun scanLine(
        language: L,
        line: String,
        state: SyntaxState,
    ): List<SyntaxPiece> {
        if (line.isEmpty()) return emptyList()
        if (language == L.NONE) return listOf(SyntaxPiece(SyntaxToken.TEXT, line))

        val pieces = mutableListOf<SyntaxPiece>()
        var at = continueFromPreviousLine(language, line, state, pieces)
        if (at < 0) return pieces

        val start = at
        while (at < line.length) {
            val ch = line[at]

            if (blockComments(language) && line.startsWith("/*", at)) {
                at = blockComment(language, line, at, state, pieces)
                if (state.inComment > 0) return pieces
                continue
            }
            if (slashComments(language) && line.startsWith("//", at)) {
                emit(pieces, SyntaxToken.COMMENT, line.substring(at))
                return pieces
            }
            if (shebangs(language) && at == 0 && line.startsWith("#!")) {
                emit(pieces, SyntaxToken.COMMENT, line.substring(at))
                return pieces
            }
            if (commentAtHash(language, line, at, start)) {
                emit(pieces, SyntaxToken.COMMENT, line.substring(at))
                return pieces
            }
            if (preprocessor(language) && ch == '#' && onlyBlankBefore(line, at)) {
                val end = identifierEnd(line, at + 1)
                emit(pieces, SyntaxToken.PREPROCESSOR, line.substring(at, end))
                at = end
                continue
            }
            if (rustRawStrings(language) && rustRawOpening(line, at) > 0) {
                at = rustRawString(line, at, state, pieces)
                if (state.inRustRawString) return pieces
                continue
            }
            if (tripleStrings(language) && tripleAt(language, line, at) != null) {
                at = tripleString(line, at, tripleAt(language, line, at)!!, state, pieces)
                if (state.inTripleString) return pieces
                continue
            }
            if (rawStrings(language) && ch == '`') {
                at = rawString(line, at, state, pieces)
                if (state.inRawString) return pieces
                continue
            }
            if (ch == '"' || (charLiterals(language) && ch == '\'')) {
                at = quoted(line, at, ch, pieces)
                continue
            }
            if (ch.isDigit() && !identifierPart(line.getOrNull(at - 1))) {
                at = number(line, at, pieces)
                continue
            }
            if (identifierStart(ch)) {
                at = word(language, line, at, pieces)
                continue
            }

            emit(pieces, SyntaxToken.TEXT, ch.toString())
            at += 1
        }
        return pieces
    }

    /**
     * Resumes a construct opened on an earlier line.
     *
     * Returns the offset to carry on from, or -1 when the construct swallows
     * this line whole.
     */
    private fun continueFromPreviousLine(
        language: L,
        line: String,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        if (state.inComment > 0) {
            val at = commentContinuation(language, line, state, pieces)
            return if (state.inComment > 0) -1 else at
        }
        if (state.inRawString) {
            val close = line.indexOf('`')
            if (close < 0) {
                emit(pieces, SyntaxToken.STRING, line)
                return -1
            }
            emit(pieces, SyntaxToken.STRING, line.substring(0, close + 1))
            state.inRawString = false
            return close + 1
        }
        if (state.inTripleString) {
            val marker = "${state.tripleQuote}".repeat(3)
            val close = line.indexOf(marker)
            if (close < 0) {
                emit(pieces, SyntaxToken.STRING, line)
                return -1
            }
            emit(pieces, SyntaxToken.STRING, line.substring(0, close + 3))
            state.inTripleString = false
            return close + 3
        }
        if (state.inRustRawString) {
            val marker = "\"" + "#".repeat(state.rustRawHashes)
            val close = line.indexOf(marker)
            if (close < 0) {
                emit(pieces, SyntaxToken.STRING, line)
                return -1
            }
            val end = close + marker.length
            emit(pieces, SyntaxToken.STRING, line.substring(0, end))
            state.inRustRawString = false
            state.rustRawHashes = 0
            return end
        }
        return 0
    }

    private fun commentContinuation(
        language: L,
        line: String,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        var at = 0
        while (at < line.length) {
            if (nestedBlockComments(language) && line.startsWith("/*", at)) {
                state.inComment += 1
                at += 2
                continue
            }
            if (line.startsWith("*/", at)) {
                state.inComment -= 1
                at += 2
                if (state.inComment <= 0) {
                    state.inComment = 0
                    emit(pieces, SyntaxToken.COMMENT, line.substring(0, at))
                    return at
                }
                continue
            }
            at += 1
        }
        emit(pieces, SyntaxToken.COMMENT, line)
        return at
    }

    private fun blockComment(
        language: L,
        line: String,
        from: Int,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        var at = from + 2
        var depth = 1
        while (at < line.length) {
            if (nestedBlockComments(language) && line.startsWith("/*", at)) {
                depth += 1
                at += 2
                continue
            }
            if (line.startsWith("*/", at)) {
                depth -= 1
                at += 2
                if (depth == 0) {
                    emit(pieces, SyntaxToken.COMMENT, line.substring(from, at))
                    return at
                }
                continue
            }
            at += 1
        }
        state.inComment = depth
        emit(pieces, SyntaxToken.COMMENT, line.substring(from))
        return line.length
    }

    private fun quoted(
        line: String,
        from: Int,
        quote: Char,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        var at = from + 1
        while (at < line.length) {
            val ch = line[at]
            if (ch == '\\') {
                at += 2
                continue
            }
            at += 1
            if (ch == quote) break
        }
        val end = minOf(at, line.length)
        emit(pieces, SyntaxToken.STRING, line.substring(from, end))
        return end
    }

    private fun rawString(
        line: String,
        from: Int,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        val close = line.indexOf('`', from + 1)
        if (close < 0) {
            state.inRawString = true
            emit(pieces, SyntaxToken.STRING, line.substring(from))
            return line.length
        }
        emit(pieces, SyntaxToken.STRING, line.substring(from, close + 1))
        return close + 1
    }

    private fun tripleAt(language: L, line: String, at: Int): Char? {
        if (line.startsWith("\"\"\"", at)) return '"'
        if (singleTripleStrings(language) && line.startsWith("'''", at)) return '\''
        return null
    }

    private fun tripleString(
        line: String,
        from: Int,
        quote: Char,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        val marker = "$quote".repeat(3)
        val close = line.indexOf(marker, from + 3)
        if (close < 0) {
            state.inTripleString = true
            state.tripleQuote = quote
            emit(pieces, SyntaxToken.STRING, line.substring(from))
            return line.length
        }
        emit(pieces, SyntaxToken.STRING, line.substring(from, close + 3))
        return close + 3
    }

    /** Returns the hash count of an `r#"` opening, or 0 when this is not one. */
    private fun rustRawOpening(line: String, at: Int): Int {
        if (line.getOrNull(at) != 'r') return 0
        if (identifierPart(line.getOrNull(at - 1))) return 0
        var cursor = at + 1
        var hashes = 0
        while (line.getOrNull(cursor) == '#') {
            hashes += 1
            cursor += 1
        }
        return if (line.getOrNull(cursor) == '"') hashes + 1 else 0
    }

    private fun rustRawString(
        line: String,
        from: Int,
        state: SyntaxState,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        val hashes = rustRawOpening(line, from) - 1
        val marker = "\"" + "#".repeat(hashes)
        val open = from + 1 + hashes + 1
        val close = line.indexOf(marker, open)
        if (close < 0) {
            state.inRustRawString = true
            state.rustRawHashes = hashes
            emit(pieces, SyntaxToken.STRING, line.substring(from))
            return line.length
        }
        val end = close + marker.length
        emit(pieces, SyntaxToken.STRING, line.substring(from, end))
        return end
    }

    private fun number(
        line: String,
        from: Int,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        var at = from
        while (at < line.length) {
            val ch = line[at]
            val exponent = (ch == '+' || ch == '-') &&
                at > from &&
                (line[at - 1] == 'e' || line[at - 1] == 'E')
            if (ch.isLetterOrDigit() || ch == '.' || ch == '_' || exponent) {
                at += 1
            } else {
                break
            }
        }
        emit(pieces, SyntaxToken.NUMBER, line.substring(from, at))
        return at
    }

    private fun word(
        language: L,
        line: String,
        from: Int,
        pieces: MutableList<SyntaxPiece>,
    ): Int {
        val end = identifierEnd(line, from)
        val text = line.substring(from, end)
        val token = when {
            text in keywords(language) -> SyntaxToken.KEYWORD
            text in types(language) -> SyntaxToken.TYPE
            text in constants(language) -> SyntaxToken.TYPE
            text in functions(language) -> SyntaxToken.FUNCTION
            callSite(line, end) -> SyntaxToken.FUNCTION
            else -> SyntaxToken.TEXT
        }
        emit(pieces, token, text)
        return end
    }

    /** An identifier immediately followed by `(` reads as a call. */
    private fun callSite(line: String, end: Int): Boolean = line.getOrNull(end) == '('

    private fun identifierEnd(line: String, from: Int): Int {
        var at = from
        while (at < line.length && identifierPart(line[at])) at += 1
        return at
    }

    private fun identifierStart(ch: Char): Boolean = ch.isLetter() || ch == '_'

    private fun identifierPart(ch: Char?): Boolean =
        ch != null && (ch.isLetterOrDigit() || ch == '_')

    private fun onlyBlankBefore(line: String, at: Int): Boolean =
        line.substring(0, at).isBlank()

    private fun commentAtHash(language: L, line: String, at: Int, start: Int): Boolean {
        if (line[at] != '#') return false
        if (escaped(line, at)) return false
        return when {
            hashComments(language) -> onlyBlankBefore(line, at)
            spacedHashComments(language) ->
                at == 0 || line[at - 1] == ' ' || line[at - 1] == '\t'
            inlineHashComments(language) -> !(language == L.MAKEFILE && at == start)
            else -> false
        }
    }

    private fun escaped(line: String, at: Int): Boolean {
        var backslashes = 0
        var cursor = at - 1
        while (cursor >= 0 && line[cursor] == '\\') {
            backslashes += 1
            cursor -= 1
        }
        return backslashes % 2 == 1
    }

    private fun emit(pieces: MutableList<SyntaxPiece>, token: SyntaxToken, text: String) {
        if (text.isEmpty()) return
        val last = pieces.lastOrNull()
        if (last != null && last.token == token) {
            pieces[pieces.lastIndex] = SyntaxPiece(token, last.text + text)
            return
        }
        pieces.add(SyntaxPiece(token, text))
    }

    private fun slashComments(l: L) =
        l == L.C || l == L.CSHARP || l == L.GO || l == L.KOTLIN ||
            l == L.RUST || l == L.V || l == L.ODIN

    private fun blockComments(l: L) = slashComments(l)

    private fun nestedBlockComments(l: L) = l == L.RUST || l == L.V || l == L.ODIN

    private fun shebangs(l: L) = l == L.V || l == L.RUBY || l == L.CRYSTAL || l == L.BASH

    private fun hashComments(l: L) = l == L.DOCKERFILE

    private fun inlineHashComments(l: L) =
        l == L.MAKEFILE || l == L.TOML || l == L.RUBY || l == L.CRYSTAL || l == L.BASH

    private fun spacedHashComments(l: L) = l == L.YAML

    private fun rawStrings(l: L) = l == L.GO || l == L.ODIN

    private fun rustRawStrings(l: L) = l == L.RUST

    private fun tripleStrings(l: L) = l == L.KOTLIN || l == L.TOML

    private fun singleTripleStrings(l: L) = l == L.TOML

    private fun charLiterals(l: L) =
        l == L.C || l == L.CSHARP || l == L.GO || l == L.KOTLIN ||
            l == L.RUST || l == L.V || l == L.ODIN

    private fun preprocessor(l: L) = l == L.C

    private fun keywords(l: L): Set<String> = when (l) {
        L.C -> Words.cKeywords
        L.CSHARP -> Words.csharpKeywords
        L.GO -> Words.goKeywords
        L.KOTLIN -> Words.kotlinKeywords
        L.DOCKERFILE -> Words.dockerfileKeywords
        L.MAKEFILE -> Words.makefileKeywords
        L.RUST -> Words.rustKeywords
        L.V -> Words.vKeywords
        L.ODIN -> Words.odinKeywords
        L.RUBY -> Words.rubyKeywords
        L.CRYSTAL -> Words.crystalKeywords
        L.BASH -> Words.bashKeywords
        else -> emptySet()
    }

    private fun types(l: L): Set<String> = when (l) {
        L.C -> Words.cTypes
        L.CSHARP -> Words.csharpTypes
        L.GO -> Words.goTypes
        L.KOTLIN -> Words.kotlinTypes
        L.RUST -> Words.rustTypes
        L.V -> Words.vTypes
        L.ODIN -> Words.odinTypes
        L.CRYSTAL -> Words.crystalTypes
        else -> emptySet()
    }

    private fun constants(l: L): Set<String> = when (l) {
        L.C -> Words.cConstants
        L.CSHARP -> Words.csharpConstants
        L.GO -> Words.goConstants
        L.KOTLIN -> Words.kotlinConstants
        L.RUST -> Words.rustConstants
        L.JSON -> Words.jsonConstants
        L.YAML -> Words.yamlConstants
        L.TOML -> Words.tomlConstants
        L.V -> Words.vConstants
        L.ODIN -> Words.odinConstants
        L.RUBY -> Words.rubyConstants
        L.CRYSTAL -> Words.crystalConstants
        else -> emptySet()
    }

    private fun functions(l: L): Set<String> = when (l) {
        L.RUBY -> Words.rubyFunctions
        L.CRYSTAL -> Words.crystalFunctions
        L.BASH -> Words.bashFunctions
        else -> emptySet()
    }
}
