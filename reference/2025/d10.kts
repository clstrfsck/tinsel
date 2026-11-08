import java.io.File

data class Machine(
    val indicators: Int,
    val buttons: List<List<Int>>,
    val joltage: List<Int>
)

fun String.extractNums() = drop(1).dropLast(1).split(",").map(String::toInt)
fun addV(a: List<Int>, b: List<Int>) = a.zip(b) { x, y -> x + y }
fun leqV(a: List<Int>, b: List<Int>) = a.zip(b).all { (x, y) -> x <= y }

fun combos(buttons: List<Int>) = buttons.fold(listOf(0 to 0)) { acc, b ->
    acc + acc.map { (m, c) -> (m xor b) to (c + 1) }
}

fun String.toMachine() = split(" ").let { parts ->
    val diagram = parts.first().filter { it == '.' || it == '#' }
    val indicators = diagram.fold(0 to 1) { (mask, bit), c ->
        (mask or if (c == '#') bit else 0) to (bit shl 1)
    }.first
    val tuples = parts.drop(1).map(String::extractNums)
    Machine(indicators, tuples.dropLast(1), tuples.last())
}

fun solve1(buttons: List<List<Int>>, indicators: Int): Int {
    val buttonMasks = buttons.map {
        it.fold(0) { acc, bit -> acc or (1 shl bit) }
    }
    val leftLen = buttonMasks.size / 2
    val leftButtons = buttonMasks.subList(0, leftLen)
    val rightButtons = buttonMasks.subList(leftLen, buttons.size)

    val left = combos(leftButtons)
        .groupBy { it.first }
        .mapValues { (_, values) -> values.minOf { it.second } }
    val right = combos(rightButtons)

    return right.mapNotNull { (rv, c) ->
        left[indicators xor rv]?.let { lc -> c + lc }
    }.minOrNull() ?: 0
}

fun solve2aux(
    goal: List<Int>,
    patterns: Map<List<Int>, List<Pair<List<Int>, Int>>>,
    memo: MutableMap<List<Int>, Int?>
): Int? {
    if (goal.all { it == 0 }) return 0
    if (memo.containsKey(goal)) return memo[goal]

    val parity = goal.map { it % 2 }
    val candidates = patterns[parity]?.filter { (pVec, _) -> leqV(pVec, goal) }

    val best = candidates?.mapNotNull { (p, cost) ->
        val nextGoal = goal.zip(p) { x, y -> (x - y) / 2 }
        solve2aux(nextGoal, patterns, memo)?.let { cost + 2 * it }
    }?.minOrNull()

    memo[goal] = best
    return best
}

fun solve2(buttons: List<List<Int>>, joltage: List<Int>): Int {
    val dim = joltage.size
    val coeffs = buttons.map { idxs -> List(dim) { if (it in idxs) 1 else 0 } }
    val patterns = (0 until coeffs.size)
        .fold(listOf(emptyList<Int>())) { acc, x -> acc + acc.map { it + x } }
        .map { idxs ->
            val vec = idxs.fold(List(dim) { 0 }) { acc, i ->
                addV(acc, coeffs[i])
            }
            val parity = vec.map { it % 2 }
            parity to (vec to idxs.size)
        }
        .groupBy { it.first }
        .mapValues { (_, pairs) -> pairs.map { it.second } }
    val memo: MutableMap<List<Int>, Int?> = mutableMapOf()

    return solve2aux(joltage, patterns, memo) ?: 0
}

val machines = File("y25d10.txt").readLines().filter { it.isNotBlank() }.map(String::toMachine)

val result1 = machines.sumOf { (inds, bms, _) -> solve1(bms, inds) }
println("Result1: $result1")

val result2 = machines.sumOf { (_, bms, joltage) -> solve2(bms, joltage)  }
println("Result2: $result2")
