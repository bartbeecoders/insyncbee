import Nav from "./components/Nav";
import Hero from "./components/Hero";
import WhatsNew from "./components/WhatsNew";
import Features from "./components/Features";
import HowItWorks from "./components/HowItWorks";
import Download from "./components/Download";
import Compare from "./components/Compare";
import FAQ from "./components/FAQ";
import Footer from "./components/Footer";

export default function App() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <WhatsNew />
        <Features />
        <HowItWorks />
        <Download />
        <Compare />
        <FAQ />
      </main>
      <Footer />
    </>
  );
}
