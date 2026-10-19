Build a quiz in `index.html`. You may add other files next to it.

The questions are in `questions.json`, next to `index.html`: a list of
`{ "question", "choices", "answer" }`, where `choices` holds four answers
and `answer` is the index (0 to 3) of the right one. Load the questions
from that file when the page opens; the file may be changed to hold any
number of questions, and the quiz must ask whatever it holds.

How it works:

- Start screen: a button with id `start` appears once the questions have
  loaded. Clicking it, or pressing Enter, starts the quiz.
- One question at a time: an element with id `question` shows its text,
  and four buttons with class `choice` show its choices, in the file's
  order. Clicking a choice, or pressing the key `1`, `2`, `3` or `4`,
  answers the question and shows the next one straight away.
- An element with id `score` shows how many questions have been answered
  right so far.
- Each question has 10 seconds. An element with id `timer` shows the whole
  seconds left, rounded up, starting at `10`. When the time runs out the
  question counts as wrong and the next one shows.
- After the last question, an element with id `results` becomes visible,
  and the questions are no longer shown. An element with id `final-score`
  shows the score as `right / total`, for example `3 / 5`. A list with id
  `review` has one `li` per question, in the order they were asked. Each
  `li` has the class `right` or `wrong`, and its text includes the question
  and its correct answer.
- A button with id `restart` on the results screen, or pressing Enter
  there, starts again from a score of 0.
- The first quiz asks the questions in the file's order. Every restart asks
  all of them again, each once, in a new random order.
