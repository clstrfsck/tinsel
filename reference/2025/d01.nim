import std/[strutils, sequtils, math, strformat]

proc positions(moves: seq[string], start: int): seq[(int, int)] =
  var
    currentC = start

  for move in moves:
    let p = currentC.euclMod(100)
    currentC = p + (if move[0] == 'L': -1 else: 1) * parseInt(move[1 .. ^1])
    result.add((p, currentC))

proc main() =
  let posns = readFile("y25d01.txt").strip().splitLines().positions(start = 50)

  let result1 = posns.countIt(it[1].euclMod(100) == 0)
  echo fmt"Result1: {result1}"

  let result2 = posns.mapIt(
    abs(it[1]) div 100 + (if it[0] != 0 and it[1] <= 0: 1 else: 0)
  ).sum
  echo fmt"Result2: {result2}"

when isMainModule:
  main()
