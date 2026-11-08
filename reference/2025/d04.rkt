#lang racket

(struct p2d (x y) #:transparent)

(define (neighbourhood p)
  (for*/list ([dy (in-range -1 2)]
              [dx (in-range -1 2)])
    (p2d (+ (p2d-x p) dx) (+ (p2d-y p) dy))))

(define (parse-input filename)
  (for*/list ([(line y) (in-indexed (file->lines filename))]
              [(c x) (in-indexed (in-string line))]
              #:when (char=? c #\@))
    (p2d x y)))

(define (count-neighbors p points-set)
  (count (λ (neighbor) (set-member? points-set neighbor))
         (neighbourhood p)))

(define (main)
  (let ([points (list->set (parse-input "y25d04.txt"))])
    (let ([result1
           (count (λ (p) (< (count-neighbors p points) 5))
                  (set->list points))])
      (displayln (format "Result1: ~a" result1)))

    (let ([result2
           (let loop ([total 0] [current-points points])
             (let ([to-remove
                    (for/set ([p (in-set current-points)]
                              #:when (< (count-neighbors p current-points) 5))
                      p)])
               (if (set-empty? to-remove)
                   total
                   (loop (+ total (set-count to-remove))
                         (set-subtract current-points to-remove)))))])
      (displayln (format "Result2: ~a" result2)))))

(main)
