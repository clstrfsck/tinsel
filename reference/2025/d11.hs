import Data.Bits ((.|.))
import Data.Map (Map, findWithDefault, fromList, keys, (!))

solve :: Map String [String] -> String -> Int -> Int
solve gr startNode startFlags = result
  where
    memo = fromList [((n, f), count n f) | n <- "out" : keys gr, f <- [0 .. 3]]
    count "out" f = f `div` 3
    count node f =
      let dacFlag = fromEnum (node == "dac")
          fftFlag = fromEnum (node == "fft") * 2
          nextFlags = f .|. dacFlag .|. fftFlag
          neighbors = findWithDefault [] node gr
       in sum [memo ! (neighbor, nextFlags) | neighbor <- neighbors]
    result = memo ! (startNode, startFlags)

main :: IO ()
main = do
  content <- readFile "y25d11.txt"
  let graph = fromList [
                (k, words $ drop 1 v)
                | line <- lines content, let (k, v) = break (== ':') line
              ]
  putStrLn $ "Result1: " ++ show (solve graph "you" 3)
  putStrLn $ "Result2: " ++ show (solve graph "svr" 0)
